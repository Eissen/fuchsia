// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// RFCOMM-specific types.
mod types;

/// The RFCOMM server that processes connections.
mod server;

pub use crate::rfcomm::{server::RfcommServer, types::ServerChannel};