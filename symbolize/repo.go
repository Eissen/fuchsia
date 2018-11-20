// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package symbolize

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"fuchsia.googlesource.com/tools/elflib"
)

// TODO(jakehehrlich): Make this private.

// BinaryFileSource is a source of binary files that can be reloaded. It might
// be an ids.txt file, it might just be a single binary file, or it might be
// another list of binaries coming from somewhere else. Currently the interface
// assumes that all binaries from a source will be files on the same system.
// Eventully we might relax this assumption.
type BinaryFileSource interface {
	// Extracts the set of binaries from this source.
	getBinaries() ([]elflib.BinaryFileRef, error)
	getModTime() (*time.Time, error)
}

// idsSource is a BinaryFileSource parsed from ids.txt
type idsSource struct {
	pathToIDs string
	rel       bool
}

func NewIDsSource(pathToIDs string, rel bool) BinaryFileSource {
	return &idsSource{pathToIDs, rel}
}

func (i *idsSource) name() string {
	return i.pathToIDs
}

func (i *idsSource) getBinaries() ([]elflib.BinaryFileRef, error) {
	file, err := os.Open(i.pathToIDs)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	out, err := elflib.ReadIDsFile(file)
	if err != nil {
		return nil, err
	}
	if i.rel {
		base := filepath.Dir(i.pathToIDs)
		for idx, ref := range out {
			out[idx].Filepath = filepath.Join(base, ref.Filepath)
		}
	}
	return out, nil
}

func (i *idsSource) getModTime() (*time.Time, error) {
	info, err := os.Stat(i.pathToIDs)
	if err != nil {
		return nil, err
	}
	out := info.ModTime()
	return &out, nil
}

type buildInfo struct {
	filepath string
	buildID  string
}

type modTimeSource struct {
	BinaryFileSource
	modTime time.Time
}

// SymbolizerRepo keeps track of build objects and source files used in those build objects.
type SymbolizerRepo struct {
	lock    sync.RWMutex
	sources []modTimeSource
	// TODO (jakehehrlich): give 'builds' a more descriptive name
	builds map[string]*buildInfo
}

func (s *SymbolizerRepo) addObject(build, filename string) {
	s.lock.Lock()
	defer s.lock.Unlock()
	s.builds[build] = &buildInfo{
		filepath: filename,
		buildID:  build,
	}
}

func NewRepo() *SymbolizerRepo {
	return &SymbolizerRepo{
		builds: make(map[string]*buildInfo),
	}
}

func (s *SymbolizerRepo) loadSource(sourcePtr *modTimeSource) error {
	source := *sourcePtr
	time, err := source.getModTime()
	if err != nil {
		return err
	}
	if !time.After(source.modTime) {
		return nil
	}
	sourcePtr.modTime = *time
	bins, err := source.getBinaries()
	if err != nil {
		return err
	}
	// TODO: Do this in paralell.
	// Verify each binary
	for _, bin := range bins {
		if err := bin.Verify(); err != nil {
			return err
		}
	}
	for _, bin := range bins {
		s.addObject(bin.BuildID, bin.Filepath)
	}
	return nil
}

// AddSource adds a source of binaries and all contained binaries.
func (s *SymbolizerRepo) AddSource(source BinaryFileSource) error {
	s.sources = append(s.sources, modTimeSource{source, time.Unix(0, 0)})
	return s.loadSource(&s.sources[len(s.sources)-1])
}

func (s *SymbolizerRepo) reloadSources() error {
	for _, source := range s.sources {
		if err := s.loadSource(&source); err != nil {
			return err
		}
	}
	return nil
}

func (s *SymbolizerRepo) readInfo(buildid string) (*buildInfo, bool) {
	s.lock.RLock()
	info, ok := s.builds[buildid]
	s.lock.RUnlock()
	return info, ok
}

// GetBuildObject lets you get the build object associated with a build ID.
func (s *SymbolizerRepo) GetBuildObject(buildid string) (string, error) {
	// No defer is used here because we don't want to hold the read lock
	// for very long.
	info, ok := s.readInfo(buildid)
	if !ok {
		// If we don't recognize that build ID, reload all sources.
		s.reloadSources()
		info, ok = s.readInfo(buildid)
		if !ok {
			// If we still don't know about that build, return an error.
			return "", fmt.Errorf("unrecognized build ID %s", buildid)
		}
	}
	return info.filepath, nil
}
