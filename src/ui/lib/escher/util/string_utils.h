// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_UTIL_STRING_UTILS_H_
#define SRC_UI_LIB_ESCHER_UTIL_STRING_UTILS_H_

#include <sstream>

namespace escher {

template <typename T>
std::string ToString(const T& obj) {
  std::ostringstream str;
  str << obj;
  return str.str();
}

}  // namespace escher

#endif  // SRC_UI_LIB_ESCHER_UTIL_STRING_UTILS_H_
