// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/lib/effects_loader/testing/effects_loader_test_base.h"

#include <dlfcn.h>

#include "src/media/audio/effects/test_effects/test_effects.h"
#include "src/media/audio/lib/effects_loader/effects_processor.h"
#include "src/media/audio/lib/effects_loader/testing/test_effects.h"

namespace media::audio::testing {

void EffectsLoaderTestBase::SetUp() {
  ::testing::Test::SetUp();
  RecreateLoader();
}

void EffectsLoaderTestBase::TearDown() {
  ASSERT_EQ(0u, test_effects_.InstanceCount());
  ASSERT_EQ(ZX_OK, test_effects_.ClearEffects());
  ::testing::Test::TearDown();
}

void EffectsLoaderTestBase::RecreateLoader() {
  ASSERT_EQ(EffectsLoader::CreateWithModule(kTestEffectsModuleName, &effects_loader_), ZX_OK);
  ASSERT_TRUE(effects_loader_);
}

}  // namespace media::audio::testing
