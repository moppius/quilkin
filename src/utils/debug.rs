/*
 * Copyright 2020 Google LLC
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
//!
//! The `debug` module is for functionality related to realtime debugging of this project.
//!

/// Attempt to convert a packet into a string, if it is one, otherwise return some human
/// readable details about the packet.
pub(crate) fn bytes_to_string(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .unwrap_or_else(|_| format!("<raw bytes :: len: {}>", bytes.len()))
}
