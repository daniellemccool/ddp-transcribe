//! Pipeline integration suite over controllable fakes. Split by concern
//! (Epic 3): fakes + fixtures in `fakes`, then one module per entry point.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fakes;
mod fetch_worker_tests;
mod pipelined_tests;
mod serial_tests;
mod transcribe_worker_tests;
