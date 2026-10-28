// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Android: the entry glue behind `crates/os-android`'s `android_main`. The private data
//! folder becomes the app's user folder (`os-paths` reads `OPENSTUDIO_USER_DIR` on every
//! platform), and the winit event loop is built on the NativeActivity's app. Everything
//! after that is the desktop run (`main_body::run_from`): one stream, one window, the same
//! project model. Touch as mouse is `crate::touch`.

use std::sync::OnceLock;

use android_activity::AndroidApp;
use jni::JavaVM;
use winit::event_loop::EventLoop;

/// The app's JVM, for native threads that ask Android for a priority (`raise_priority`), set at entry.
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// The JVM `raise_priority` uses; `None` before `run` (and a desktop build never asks).
pub(crate) fn audio_vm() -> Option<&'static JavaVM> {
    JAVA_VM.get()
}

/// The NativeActivity's app thread runs this: the app lives as long as the activity does.
pub fn run(app: AndroidApp) {
    // The app's files live in the install's private folder, so no storage permission. Set before
    // anything reads a path (the first read is `App::new`'s settings, in `run_from`).
    if let Some(data) = app.internal_data_path().filter(|p| p.is_absolute()) {
        let dir = data.join("user");
        let _ = std::fs::create_dir_all(&dir);
        // Edition 2024: setting the environment is unsafe (threads may be reading it; none started yet).
        unsafe { std::env::set_var("OPENSTUDIO_USER_DIR", &dir) };
        // Cursor themes and other HOME readers look in the app's folder; whatever they miss is skipped.
        unsafe { std::env::set_var("HOME", &data) };
    }
    // A crash's log is also written here (`crash_log` reads it), because the private folder is reachable only over
    // adb; a file manager opens the external one, so the launch crash is readable on the device itself.
    if let Some(ext) = app.external_data_path().filter(|p| p.is_absolute()) {
        let _ = std::fs::create_dir_all(&ext);
        unsafe { std::env::set_var("OPENSTUDIO_LOG_DIR", &ext) };
    }
    crate::log_panics();
    // The feeder thread raises its priority through the JVM (`raise_priority`): keep the handle. The VM lives as long
    // as the process, and the pointer is the app's own (android-activity keeps it in its state).
    // SAFETY: the pointer is the live JVM of this process, still owned by android-activity.
    if let Ok(vm) = (unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }) {
        let _ = JAVA_VM.set(vm);
    }
    let mut builder = EventLoop::<()>::with_user_event();
    use winit::platform::android::EventLoopBuilderExtAndroid;
    builder.with_android_app(app);
    crate::run_from(builder, None, true);
}
