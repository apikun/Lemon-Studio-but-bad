// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Android: the entry glue behind `crates/os-android`'s `android_main`. The private data
//! folder becomes the app's user folder (`os-paths` reads `OPENSTUDIO_USER_DIR` on every
//! platform), and the winit event loop is built on the NativeActivity's app. Everything
//! after that is the desktop run (`main_body::run_from`): one stream, one window, the same
//! project model. Touch as mouse is `crate::touch`.

use android_activity::AndroidApp;
use winit::event_loop::EventLoop;

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
    crate::log_panics();
    let mut builder = EventLoop::<()>::with_user_event();
    use winit::platform::android::EventLoopBuilderExtAndroid;
    builder.with_android_app(app);
    crate::run_from(builder, None, true);
}
