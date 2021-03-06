#!/bin/bash
# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -o errexit    # exit when a command fails
set -o nounset    # error when an undefined variable is referenced
set -o pipefail   # error if the input command to a pipe fails

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
readonly FUCHSIA_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"

# The go repos that fint depends on, given an an array of pairs of the form
# (<repo name>, <path in the tree to that vendored repo>).
# Note: fint can use any packages in //tools without declaring them here,
# because fint is part of the //tools Go module. If fint ever depends on
# packages from other top-level directories from the Fuchsia root, the
# modules corresponding to those top-level directories must be added here.
readonly GO_DEPS=(
  "github.com/golang/protobuf"
  "${FUCHSIA_ROOT}/third_party/golibs/github.com/golang/protobuf"

  "github.com/google/subcommands"
  "${FUCHSIA_ROOT}/third_party/golibs/github.com/google/subcommands"

  "google.golang.org/protobuf"
  "${FUCHSIA_ROOT}/third_party/golibs/github.com/protocolbuffers/protobuf-go"
)

print_usage_and_exit() {
  readonly exit_code=$1
  # In the case of a non-zero exit, redirect the stdout below to stderr.
  if [[ $exit_code -ne 0 ]]; then
    exec 1 >&2
  fi

  echo ""
  echo "Used to bootstrap the \"fint\" tool, which provides the platform's"
  echo "\"integration interface\"."
  echo "See //tools/integration/README.md for more details."
  echo ""
  echo "usage: $(basename "$0") (-o <path>)"
  echo ""
  echo "options:"
  echo "-o: points to a file path at which to output fint; if unsupplied, it"
  echo "    will be output to the current working directory."
  echo ""

  exit "$exit_code"
}


###############################################################################
# Returns the host platform, of the form <OS>-<architecture>.
# Globals:
#   None
# Arguments:
#   None
# Returns:
#   The host platform, if successful.
###############################################################################
host_platform() {
  readonly uname="$(uname -s -m)"
  case "${uname}" in
    "Linux x86_64") echo linux-x64 ;;
    "Darwin x86_64") echo mac-x64 ;;
    *)
      echo "unsupported infrastructure platform: ${uname}" 1>&2
      exit 1
      ;;
  esac
}

###############################################################################
# Symlinks already vendored go repositories, depended on by fint, to a provided
# go path.
# Globals:
#   GO_DEPS
# Arguments:
#   $1 - a go path
# Returns:
#   None
###############################################################################
symlink_go_deps() {
  readonly gopath="$1"
  for (( i=0 ; i < ${#GO_DEPS[@]} ; i += 2 )) ; do
    host="${GO_DEPS[i]}"
    src="${GO_DEPS[i+1]}"
    dest="${gopath}/src/${host}"
    mkdir -p "$(dirname "${dest}")"
    if [[ -d "${dest}" ]]; then
      echo "cannot symlink deps: ${dest} already exists"
    fi
    ln -s "${src}" "${dest}"
  done
}

# The `realpath` command is not available on all systems, so we reimplement it
# here in pure bash. It converts relative paths to absolute, and leaves
# absolute paths as-is.
realpath() {
    [[ $1 = /* ]] && echo "$1" || echo "$PWD/${1#./}"
}

main() {
  local output
  output="$(pwd)/fint"
  while getopts 'ho:' opt; do
    case "$opt" in
      h) print_usage_and_exit 0 ;;
      o) output=$(realpath "${OPTARG}") ;;
      ?) print_usage_and_exit 1  ;;
    esac
  done

  readonly GOPATH=$(mktemp -d "${TMPDIR:-/tmp}/fint.XXXXX")
  rm_gopath() {
    rm -r -f "${GOPATH}"
  }
  trap rm_gopath EXIT
  export GOPATH
  symlink_go_deps "${GOPATH}"
  # Execute `go build` from the fuchsia root, as the package to build must be
  # supplied as a relative path.
  readonly go_bin="${FUCHSIA_ROOT}/prebuilt/third_party/go/$(host_platform)/bin/go"
  # Go commands must be run in //tools, because each top-level directory is its
  # own Go module.
  cd "${FUCHSIA_ROOT}/tools" && ${go_bin} build -o "${output}" "./integration/cmd/fint"
}

main "$@"
