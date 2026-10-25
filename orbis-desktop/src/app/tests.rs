use super::*;
use std::{fs, time::Instant};

use crate::features;

fn finish(app: &mut OrbisApp) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.job.is_some() {
        assert!(Instant::now() < deadline, "desktop worker did not finish");
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn passcodes_are_accepted_only_after_validation_and_cleared_for_another_package() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.pkg");
    fs::write(&path, features::passcoded_pkg(2)).unwrap();
    let mut app = OrbisApp::default();
    app.open(path.clone());
    finish(&mut app);
    assert_eq!(app.package.as_ref().unwrap().missing_keys, 1);
    app.passcode_input = String::from_utf8(features::PASSCODE.to_vec())
        .unwrap()
        .into();
    app.validate_passcode();
    assert!(app.passcode.is_none());
    assert!(app.passcode_input.is_empty());
    finish(&mut app);
    assert!(app.passcode.is_some());
    assert!(app.pending_passcode.is_none());
    assert_eq!(app.package.as_ref().unwrap().missing_keys, 0);
    app.passcode_input = "x".repeat(32).into();
    app.validate_passcode();
    finish(&mut app);
    assert!(app.notice.as_ref().unwrap().attention);
    assert!(app.pending_passcode.is_none());
    // A rejected replacement does not discard the previous validated value.
    assert!(app.passcode.is_some());
    app.start(
        path.clone(),
        Operation::Verify {
            metadata_only: true,
        },
    );
    finish(&mut app);
    assert!(
        app.verification
            .as_ref()
            .unwrap()
            .readability
            .as_ref()
            .unwrap()
            .skipped
            .is_empty()
    );
    app.clear_passcode();
    finish(&mut app);
    assert!(app.passcode.is_none());
    assert!(app.verification.is_none());
    assert_eq!(app.package.as_ref().unwrap().missing_keys, 1);
    app.passcode_input = String::from_utf8(features::PASSCODE.to_vec())
        .unwrap()
        .into();
    app.validate_passcode();
    finish(&mut app);
    assert!(app.passcode.is_some());
    app.open(path);
    assert!(app.passcode.is_none());
    assert!(app.verification.is_none());
    finish(&mut app);
    assert_eq!(app.package.as_ref().unwrap().missing_keys, 1);
}

#[test]
fn cancelled_verification_keeps_a_report_and_resets_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signed.pkg");
    fs::write(&path, features::signed_pkg()).unwrap();
    let mut app = OrbisApp::default();
    app.open(path.clone());
    finish(&mut app);
    app.start(
        path,
        Operation::Verify {
            metadata_only: false,
        },
    );
    app.job.as_ref().unwrap().cancel();
    finish(&mut app);
    assert_eq!(app.verification.as_ref().unwrap().status, "interrupted");
    assert!(app.notice.as_ref().unwrap().attention);
    assert!(app.job.is_none());
}

fn passcode_frame(app: &mut OrbisApp, events: Vec<egui::Event>) {
    let ctx = app.ctx.clone();
    // Advance beyond egui's undo debounce without sleeping.
    let time = ctx.input(|input| input.time) + 2.0;
    ctx.memory_mut(|memory| {
        memory.set_everything_is_visible(true);
        memory.request_focus(OrbisApp::passcode_input_id());
    });
    let mut output = ctx.run_ui(
        egui::RawInput {
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| app.passcode_settings(ui),
    );
    // This headless test has no renderer to consume texture updates.
    output.textures_delta.clear();
    assert!(
        ctx.read_response(OrbisApp::passcode_input_id())
            .unwrap()
            .has_focus()
    );
}

fn passcode_undo_key(redo: bool) -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Z,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            command: true,
            shift: redo,
            ..Default::default()
        },
    }
}

fn assert_passcode_editor_reset(value: &str, reset: impl FnOnce(&mut OrbisApp)) {
    let mut app = OrbisApp::default();
    passcode_frame(&mut app, vec![]);
    passcode_frame(&mut app, vec![egui::Event::Paste(value.into())]);
    passcode_frame(&mut app, vec![]);
    assert_eq!(app.passcode_input.as_str(), value);

    // Prove that the real password widget has captured the pasted passcode.
    passcode_frame(&mut app, vec![passcode_undo_key(false)]);
    assert!(app.passcode_input.is_empty());
    passcode_frame(&mut app, vec![passcode_undo_key(true)]);
    assert_eq!(app.passcode_input.as_str(), value);
    let saved_memory = app.ctx.memory(Clone::clone);

    reset(&mut app);
    assert!(app.passcode_input.is_empty());
    assert!(egui::TextEdit::load_state(&app.ctx, OrbisApp::passcode_input_id()).is_none());
    finish(&mut app);
    passcode_frame(&mut app, vec![passcode_undo_key(false)]);
    assert!(app.passcode_input.is_empty());
    passcode_frame(&mut app, vec![passcode_undo_key(true)]);
    assert!(app.passcode_input.is_empty());

    // Restoring a pre-reset Memory clone must not resurrect its shared history.
    app.ctx.memory_mut(|memory| *memory = saved_memory);
    passcode_frame(&mut app, vec![passcode_undo_key(false)]);
    assert!(app.passcode_input.is_empty());
    passcode_frame(&mut app, vec![passcode_undo_key(true)]);
    assert!(app.passcode_input.is_empty());

    // New pastes still support undo/redo, without reaching the prior passcode.
    let replacement = "n".repeat(32);
    passcode_frame(&mut app, vec![egui::Event::Paste(replacement.clone())]);
    passcode_frame(&mut app, vec![]);
    assert_eq!(app.passcode_input.as_str(), replacement);
    for _ in 0..3 {
        passcode_frame(&mut app, vec![passcode_undo_key(false)]);
        assert!(app.passcode_input.is_empty());
    }
    passcode_frame(&mut app, vec![passcode_undo_key(true)]);
    assert_eq!(app.passcode_input.as_str(), replacement);
}

#[test]
fn passcode_editor_forgets_history_after_validation() {
    assert_passcode_editor_reset("0123456789abcdef0123456789abcdef", |app| {
        app.validate_passcode();
        assert!(app.pending_passcode.is_some());
    });
}

#[test]
fn passcode_editor_forgets_history_after_rejected_input() {
    // The UI enables validation for 32 bytes, but Passcode rejects non-ASCII.
    assert_passcode_editor_reset(&"é".repeat(16), |app| {
        app.validate_passcode();
        assert!(app.pending_passcode.is_none());
        assert!(app.notice.as_ref().unwrap().attention);
    });
}

#[test]
fn passcode_editor_forgets_history_after_clear() {
    assert_passcode_editor_reset("0123456789abcdef0123456789abcdef", |app| {
        app.clear_passcode();
    });
}

#[test]
fn passcode_editor_forgets_history_after_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("next.pkg");
    fs::write(&path, features::signed_pkg()).unwrap();
    assert_passcode_editor_reset("0123456789abcdef0123456789abcdef", |app| {
        app.open(path);
    });
}
