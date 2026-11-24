use super::*;
use eframe::App;
use egui::accesskit::{Action as AccessibilityAction, ActionRequest, TreeId, TreeUpdate};
use std::{fs, time::Instant};

use crate::features;

pub(super) fn finish(app: &mut OrbisApp) {
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
    assert_eq!(app.package.as_ref().unwrap().update.missing_keys, 1);
    app.passcode_input = String::from_utf8(features::PASSCODE.to_vec())
        .unwrap()
        .into();
    app.validate_passcode();
    assert!(app.passcode.is_none());
    assert!(app.passcode_input.is_empty());
    finish(&mut app);
    assert!(app.passcode.is_some());
    assert!(app.pending_passcode.is_none());
    assert_eq!(app.package.as_ref().unwrap().update.missing_keys, 0);
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
    assert!(
        app.package
            .as_ref()
            .unwrap()
            .update
            .readiness
            .has_update_metadata()
    );
    app.action = Action::Update;
    app.clear_passcode();
    finish(&mut app);
    assert!(
        !app.package
            .as_ref()
            .unwrap()
            .update
            .readiness
            .has_update_metadata()
    );
    assert!(app.action == Action::Extract);
    assert!(app.passcode.is_none());
    assert!(app.verification.is_none());
    assert_eq!(app.package.as_ref().unwrap().update.missing_keys, 1);
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
    assert_eq!(app.package.as_ref().unwrap().update.missing_keys, 1);
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
        |ui| app.passcode_settings(ui, false),
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

pub(super) fn frame(app: &mut OrbisApp, mut input: egui::RawInput) -> TreeUpdate {
    let ctx = app.ctx.clone();
    ctx.enable_accesskit();
    input.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(640.0, 560.0),
    ));
    let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()));
    output.textures_delta.clear();
    output.platform_output.accesskit_update.unwrap()
}

pub(super) fn click(app: &mut OrbisApp, label: &str) {
    let tree = frame(app, Default::default());
    let (button, _) = tree
        .nodes
        .iter()
        // An action tab can have the same label as the footer button.
        .filter(|(_, node)| node.label() == Some(label))
        .min_by_key(|(_, node)| node.toggled().is_some())
        .unwrap_or_else(|| panic!("missing button {label}"));
    let event = egui::Event::AccessKitActionRequest(ActionRequest {
        action: AccessibilityAction::Click,
        target_tree: TreeId::ROOT,
        target_node: *button,
        data: None,
    });
    frame(
        app,
        egui::RawInput {
            events: vec![event],
            ..Default::default()
        },
    );
}

#[test]
fn extraction_form_with_a_metadata_warning_fits_the_default_window() {
    let ctx = egui::Context::default();
    let mut app = OrbisApp::new(&ctx, None);
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .join("Sample.Game.of.the.Year.Edition.01.00.pkg");
    fs::write(&path, features::signed_pkg_with_metadata(true)).unwrap();
    app.open(path);
    finish(&mut app);

    // Let the panels reserve their final sizes before measuring the scroll area.
    frame(&mut app, Default::default());
    let tree = frame(&mut app, Default::default());
    assert!(tree.nodes.iter().any(|(_, node)| {
        node.value()
            .is_some_and(|text| text.contains("encrypted metadata entry cannot be read"))
    }));
    assert!(
        tree.nodes
            .iter()
            .any(|(_, node)| node.label() == Some("Resume an unfinished extraction")),
        "the last extraction setting should be visible on opening the PKG"
    );
    assert!(
        !tree
            .nodes
            .iter()
            .any(|(_, node)| node.role() == egui::accesskit::Role::ScrollBar),
        "the collapsed extraction form should fit without scrolling"
    );
}

#[test]
fn byte_count_stays_outside_the_progress_bar_in_both_themes() {
    use egui::accesskit::Role;
    use orbis_cli::ExtractProgress;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.pkg");
    fs::write(&path, features::signed_pkg()).unwrap();
    let ctx = egui::Context::default();
    let mut app = OrbisApp::new(&ctx, Some(path));
    // Render successive progress snapshots before the app polls the worker.
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.set_theme(theme);
        for done in [0, 8, 100] {
            let progress = &app.job.as_ref().unwrap().progress;
            progress.bytes_total(100);
            progress.bytes_processed(done);
            let tree = frame(&mut app, Default::default());
            let (_, bar) = tree
                .nodes
                .iter()
                .find(|(_, node)| {
                    node.role() == Role::ProgressIndicator && node.numeric_value().is_some()
                })
                .unwrap();
            let expected = format!("{} / 100.0 B", human_size(done));
            let (id, label) = tree
                .nodes
                .iter()
                .find(|(_, node)| {
                    node.role() == Role::Label && node.value() == Some(expected.as_str())
                })
                .unwrap();
            assert!(label.bounds().unwrap().x1 <= bar.bounds().unwrap().x0);
            assert_eq!(bar.labelled_by(), &[*id]);
        }
    }
}

#[derive(Debug)]
pub(super) struct DroppedPath(pub PathBuf);

impl egui::DroppedFile for DroppedPath {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        panic!("a dropped native folder should be opened by path")
    }
}

#[test]
fn extracted_game_can_preview_retry_and_install_an_update() {
    use crate::common;
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("CUSA00000");
    let base_pkg = temp.path().join("base.pkg");
    let mut bytes = common::update("01.00", "game.txt", b"BASE");
    let sfo = common::sfo("CUSA00000", "gd", "01.00");
    bytes[0x1a00..0x1a00 + sfo.len()].copy_from_slice(&sfo);
    common::be32(&mut bytes, 0x78, 0);
    fs::write(&base_pkg, bytes).unwrap();
    let mut app = OrbisApp::default();
    app.open(base_pkg);
    finish(&mut app);
    click(&mut app, "Extract files");
    finish(&mut app);
    click(&mut app, "Open game folder in Orbis");
    finish(&mut app);
    assert_eq!(app.game.as_ref().unwrap().base, base);
    assert!(
        frame(&mut app, Default::default())
            .nodes
            .iter()
            .any(|(_, n)| n.label() == Some("Install update…"))
    );

    let wrong = temp.path().join("other-game.pkg");
    let mut bytes = common::update("01.12", "game.txt", b"OTHER");
    let sfo = common::sfo("CUSA12345", "gp", "01.12");
    bytes[0x1a00..0x1a00 + sfo.len()].copy_from_slice(&sfo);
    bytes[0x47..0x50].copy_from_slice(b"CUSA12345");
    fs::write(&wrong, bytes).unwrap();
    app.open_package(wrong, UpdateBase::Game(base.clone()));
    finish(&mut app);
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .details
            .contains("does not match")
    );
    assert!(app.preview.is_none());
    assert!(
        frame(&mut app, Default::default())
            .nodes
            .iter()
            .any(|(_, n)| n.label() == Some("Choose another PKG…"))
    );

    let update = temp.path().join("update.pkg");
    fs::write(&update, common::update("01.12", "game.txt", b"PATCH")).unwrap();
    frame(
        &mut app,
        egui::RawInput {
            dropped_files: vec![Arc::new(DroppedPath(update))],
            ..Default::default()
        },
    );
    finish(&mut app);
    let plan = app.preview.as_ref().unwrap();
    assert_eq!(plan.current_version, "01.00");
    assert_eq!(plan.update_version, "01.12");
    assert_eq!(
        plan.destination,
        base.canonicalize()
            .unwrap()
            .with_file_name("CUSA00000-UPDATE")
    );
    assert!(!plan.destination.exists());
    click(&mut app, "Install update");
    finish(&mut app);
    click(&mut app, "Done");
    finish(&mut app);
    let game = app.game.as_ref().unwrap();
    assert_eq!(game.base, base);
    assert_eq!(
        game.status.as_ref().unwrap().active_version.as_deref(),
        Some("01.12")
    );
    assert_eq!(fs::read(base.join("game.txt")).unwrap(), b"BASE");
    assert_eq!(
        fs::read(base.with_file_name("CUSA00000-UPDATE").join("game.txt")).unwrap(),
        b"PATCH"
    );
}

#[test]
fn replacing_a_pkg_in_the_same_game_rechecks_the_selected_install_mode() {
    let temp = tempfile::tempdir().unwrap();
    let base = crate::common::base_game(temp.path());
    let first = temp.path().join("first.pkg");
    let replacement = temp.path().join("replacement.pkg");
    fs::write(&first, crate::common::update("01.10", "base.txt", b"FIRST")).unwrap();
    fs::write(
        &replacement,
        crate::common::update("01.12", "base.txt", b"SECOND"),
    )
    .unwrap();
    let mut app = OrbisApp::default();
    app.open_package(first, UpdateBase::Game(base.clone()));
    finish(&mut app);
    click(&mut app, "Merge into game folder");
    finish(&mut app);
    assert_eq!(app.preview.as_ref().unwrap().mode, InstallMode::Merge);

    app.open_package(replacement.clone(), UpdateBase::Game(base.clone()));
    assert!(app.preview.is_none());
    finish(&mut app);
    let plan = app.preview.as_ref().unwrap();
    assert_eq!(plan.mode, InstallMode::Merge);
    assert_eq!(plan.update_version, "01.12");
    assert_eq!(plan.destination, base.canonicalize().unwrap());
    assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");

    app.open(replacement);
    finish(&mut app);
    assert_eq!(app.mode, InstallMode::Separate);
}

fn assert_rejected_locked_update(app: &mut OrbisApp, reason: &str) {
    assert_eq!(app.package.as_ref().unwrap().update.missing_keys, 1);
    assert!(app.preview.is_none());
    let replace = match app.base {
        UpdateBase::Game(_) => "Choose another PKG…",
        UpdateBase::Pick(_) => "Browse…",
    };
    assert!(app.action == Action::Update);
    let tree = frame(app, Default::default());
    assert!(
        tree.nodes
            .iter()
            .any(|(_, n)| n.value().is_some_and(|v| v.contains(reason))),
        "expected {reason}"
    );
    assert!(
        tree.nodes
            .iter()
            .any(|(_, n)| n.label() == Some(replace) && !n.is_disabled()),
        "expected {reason}"
    );
    assert!(
        !tree.nodes.iter().any(|(_, n)| matches!(
            n.label(),
            Some("Validate passcode" | "Package passcode required")
        )),
        "unexpected passcode prompt for {reason}"
    );
}

#[test]
fn incompatible_packages_with_locked_entries_offer_a_replacement_without_a_passcode() {
    use crate::common;
    use orbis_pkg::entry::EntryId;

    let temp = tempfile::tempdir().unwrap();
    let base = common::base_game(temp.path());
    let mut app = OrbisApp::default();

    let mut base_pkg = features::passcoded_pkg_with_entry(2, EntryId::PfsImageKey);
    common::be32(&mut base_pkg, 0x78, 0);
    let mut delta = features::passcoded_pkg(2);
    common::be32(&mut delta, 0x74, 0x1e);
    let mut wrong_category = features::passcoded_pkg_with_entry(2, EntryId::PfsImageKey);
    let sfo = common::sfo("CUSA00000", "gd", "01.12");
    wrong_category[0x1a00..0x1a00 + sfo.len()].copy_from_slice(&sfo);
    let mut unsupported_sfo = features::passcoded_pkg(2);
    common::be32(&mut unsupported_sfo, 0x104c, 7 << 12);
    let mut unsupported_entry = features::passcoded_pkg_with_entry(2, EntryId::PfsImageKey);
    common::be32(&mut unsupported_entry, 0x102c, 7 << 12);

    let mut missing_keys = features::passcoded_pkg(2);
    common::be32(&mut missing_keys, 0x1000, 0x16); // No EntryKeys record.

    for (file, bytes, reason) in [
        ("base.pkg", base_pkg, "expected a PS4 full-file"),
        ("delta.pkg", delta, "binary delta packages"),
        ("category.pkg", wrong_category, "CATEGORY must be gp"),
        ("missing-keys.pkg", missing_keys, "missing entry-key record"),
        (
            "unsupported-sfo.pkg",
            unsupported_sfo,
            "unsupported key slot 7",
        ),
        (
            "unsupported-entry.pkg",
            unsupported_entry,
            "unsupported key slot 7",
        ),
    ] {
        let path = temp.path().join(file);
        fs::write(&path, bytes).unwrap();
        app.open_package(path, UpdateBase::Game(base.clone()));
        finish(&mut app);
        assert_rejected_locked_update(&mut app, reason);
    }
}

#[test]
fn locked_updates_for_another_edition_are_rejected_before_requesting_a_passcode() {
    use crate::common;
    let temp = tempfile::tempdir().unwrap();
    let base = common::base_game(temp.path());
    let mut metadata = common::sfo("CUSA00000", "gd", "01.00");
    let region = metadata
        .windows(7)
        .position(|bytes| bytes == b"UP0000-")
        .unwrap();
    metadata[region..region + 2].copy_from_slice(b"EP");
    fs::write(base.join("sce_sys/param.sfo"), metadata).unwrap();

    let mut app = OrbisApp::default();
    for entry in [
        orbis_pkg::entry::EntryId::ParamSfo,
        orbis_pkg::entry::EntryId::PfsImageKey,
    ] {
        let path = temp.path().join("locked.pkg");
        fs::write(&path, features::passcoded_pkg_with_entry(2, entry)).unwrap();
        app.open_package(path, UpdateBase::Game(base.clone()));
        finish(&mut app);
        assert_rejected_locked_update(&mut app, "does not match");
    }
}

#[test]
fn choosing_a_different_base_rechecks_the_locked_package() {
    use crate::common;
    let temp = tempfile::tempdir().unwrap();
    let base = common::base_game(temp.path());
    let wrong = temp.path().join("another-game");
    fs::create_dir_all(wrong.join("sce_sys")).unwrap();
    fs::write(
        wrong.join("sce_sys/param.sfo"),
        common::sfo("CUSA12345", "gd", "01.00"),
    )
    .unwrap();
    let path = temp.path().join("locked.pkg");
    fs::write(
        &path,
        features::passcoded_pkg_with_entry(2, orbis_pkg::entry::EntryId::PfsImageKey),
    )
    .unwrap();

    let mut app = OrbisApp::default();
    app.open(path);
    finish(&mut app);
    app.base = UpdateBase::Pick(Some(wrong));
    app.update_target_changed();
    finish(&mut app);
    assert_rejected_locked_update(&mut app, "does not match");

    let view = frame(&mut app, Default::default());
    for label in ["Extract files", "Verify package"] {
        assert!(
            view.nodes
                .iter()
                .any(|(_, node)| node.label() == Some(label) && !node.is_disabled())
        );
    }

    app.base = UpdateBase::Pick(Some(base));
    app.update_target_changed();
    finish(&mut app);
    app.passcode_input = String::from_utf8(features::PASSCODE.to_vec())
        .unwrap()
        .into();
    click(&mut app, "Validate passcode");
    finish(&mut app);
    click(&mut app, "Check update");
    finish(&mut app);
    assert_eq!(app.preview.as_ref().unwrap().update_version, "01.12");
}

#[test]
fn locked_updates_check_the_active_version_in_either_open_order() {
    use crate::common;
    use orbis_cli::{SilentProgress, install};
    for installed in ["01.12", "01.15"] {
        let temp = tempfile::tempdir().unwrap();
        let base = common::base_game(temp.path());
        let pkg = orbis_pkg::Pkg::new(common::update(installed, "game.txt", b"ACTIVE")).unwrap();
        install::install(&pkg, &base, SilentProgress).unwrap();
        let path = temp.path().join("locked.pkg");
        fs::write(
            &path,
            features::passcoded_pkg_with_entry(2, orbis_pkg::entry::EntryId::PfsImageKey),
        )
        .unwrap();
        let reason = format!("01.12 is not newer than installed {installed}");
        let mut app = OrbisApp::default();
        app.open_package(path.clone(), UpdateBase::Game(base.clone()));
        finish(&mut app);
        assert_rejected_locked_update(&mut app, &reason);
        app.open(path);
        finish(&mut app);
        app.base = UpdateBase::Pick(Some(base));
        app.update_target_changed();
        finish(&mut app);
        assert_rejected_locked_update(&mut app, &reason);
    }
}

#[test]
fn returning_to_a_locked_pkg_after_rollback_rechecks_the_active_version() {
    use crate::common;
    use orbis_cli::{SilentProgress, install};
    let temp = tempfile::tempdir().unwrap();
    let base = common::base_game(temp.path());
    let active = orbis_pkg::Pkg::new(common::update("01.12", "game.txt", b"ACTIVE")).unwrap();
    install::install(&active, &base, SilentProgress).unwrap();
    let path = temp.path().join("locked.pkg");
    fs::write(
        &path,
        features::passcoded_pkg_with_entry(2, orbis_pkg::entry::EntryId::PfsImageKey),
    )
    .unwrap();
    let mut app = OrbisApp::default();
    app.open_package(path, UpdateBase::Pick(Some(base.clone())));
    finish(&mut app);
    assert_rejected_locked_update(&mut app, "01.12 is not newer than installed 01.12");

    app.open_game(base);
    finish(&mut app);
    click(&mut app, "Restore base 01.00…");
    click(&mut app, "Confirm restore");
    finish(&mut app);
    click(&mut app, "Done");
    finish(&mut app);
    click(&mut app, "Back");
    finish(&mut app);
    app.passcode_input = String::from_utf8(features::PASSCODE.to_vec())
        .unwrap()
        .into();
    click(&mut app, "Validate passcode");
    finish(&mut app);
    click(&mut app, "Check update");
    finish(&mut app);
    assert_eq!(app.preview.as_ref().unwrap().current_version, "01.00");
}

#[test]
fn switching_install_modes_rechecks_a_locked_update_in_both_open_orders() {
    use crate::common;
    use orbis_cli::{SilentProgress, install};
    let temp = tempfile::tempdir().unwrap();
    let base = common::base_game(temp.path());
    let active = orbis_pkg::Pkg::new(common::update("01.10", "game.txt", b"ACTIVE")).unwrap();
    install::install(&active, &base, SilentProgress).unwrap();
    let path = temp.path().join("locked.pkg");
    use orbis_pkg::entry::EntryId;
    for entry in [EntryId::ParamSfo, EntryId::PfsImageKey] {
        fs::write(&path, features::passcoded_pkg_with_entry(2, entry)).unwrap();
        for selection in [
            UpdateBase::Game(base.clone()),
            UpdateBase::Pick(Some(base.clone())),
        ] {
            let mut app = OrbisApp::default();
            app.open_package(path.clone(), selection);
            finish(&mut app);
            if app.action != Action::Update {
                click(&mut app, "Install update");
            }
            click(&mut app, "Merge into game folder");
            finish(&mut app);
            assert_rejected_locked_update(&mut app, "merge requires an original base");
            click(&mut app, "Keep update separate");
            finish(&mut app);
            let tree = frame(&mut app, Default::default());
            assert!(
                tree.nodes
                    .iter()
                    .any(|(_, n)| n.label() == Some("Validate passcode"))
            );
        }
    }
}

#[test]
fn target_warnings_show_the_field_that_failed_validation() {
    use crate::common;
    let temp = tempfile::tempdir().unwrap();
    let base = common::base_game(temp.path());
    fs::write(
        base.join("sce_sys/param.sfo"),
        common::sfo("CUSA00000", "gd", "bad"),
    )
    .unwrap();
    let path = temp.path().join("locked.pkg");
    fs::write(&path, features::passcoded_pkg(2)).unwrap();
    let mut app = OrbisApp::default();
    app.open_package(path, UpdateBase::Game(base));
    finish(&mut app);
    assert_rejected_locked_update(&mut app, "APP_VER must use NN.NN");
}

#[test]
fn a_locked_update_version_is_checked_after_the_passcode_reveals_it() {
    use crate::common;
    use orbis_cli::{SilentProgress, install};
    let temp = tempfile::tempdir().unwrap();
    let base = common::base_game(temp.path());
    let installed = orbis_pkg::Pkg::new(common::update("01.15", "game.txt", b"ACTIVE")).unwrap();
    install::install(&installed, &base, SilentProgress).unwrap();
    let path = temp.path().join("locked-sfo.pkg");
    fs::write(&path, features::passcoded_pkg(2)).unwrap();
    let mut app = OrbisApp::default();
    app.open_package(path, UpdateBase::Game(base));
    finish(&mut app);
    app.passcode_input = String::from_utf8(features::PASSCODE.to_vec())
        .unwrap()
        .into();
    click(&mut app, "Validate passcode");
    finish(&mut app);
    assert!(app.preview.is_none());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .details
            .contains("01.12 is not newer than installed 01.15")
    );
}

#[test]
fn update_with_locked_metadata_requires_a_passcode_even_with_readable_sfo() {
    let temp = tempfile::tempdir().unwrap();
    let base = crate::common::base_game(temp.path());
    let path = temp.path().join("locked-metadata.pkg");
    let bytes = features::passcoded_pkg_with_entry(2, orbis_pkg::entry::EntryId::PfsImageKey);
    fs::write(&path, bytes).unwrap();
    let mut app = OrbisApp::default();
    app.open_package(path.clone(), UpdateBase::Game(base.clone()));
    finish(&mut app);
    let info = app.package.as_ref().unwrap();
    assert!(info.update.readiness.has_update_metadata());
    assert_eq!(info.update.missing_keys, 1);
    assert!(app.preview.is_none());
    let tree = frame(&mut app, Default::default());
    assert!(
        tree.nodes
            .iter()
            .any(|(_, node)| node.value().is_some_and(|value| {
                value.contains("1 encrypted metadata entry cannot be read")
                    && value.contains("passcode")
            }))
    );
    assert!(
        tree.nodes
            .iter()
            .any(|(_, node)| node.label() == Some("Validate passcode"))
    );
    assert!(
        tree.nodes
            .iter()
            .any(|(_, node)| node.label() == Some("Check update") && node.is_disabled())
    );

    let error = operations::run(
        &path,
        Operation::PreviewUpdate {
            base: base.clone(),
            mode: InstallMode::Separate,
        },
        &orbis_cli::SilentProgress,
    )
    .unwrap_err();
    assert!(error.details.contains("decryption key for slot 2"));
    assert!(!temp.path().join(".CUSA00000.orbis-install").exists());

    app.passcode_input = String::from_utf8(features::PASSCODE.to_vec())
        .unwrap()
        .into();
    click(&mut app, "Validate passcode");
    finish(&mut app);
    assert_eq!(app.package.as_ref().unwrap().update.missing_keys, 0);
    assert_eq!(app.preview.as_ref().unwrap().update_version, "01.12");
    click(&mut app, "Install update");
    finish(&mut app);
    assert_eq!(app.notice.as_ref().unwrap().title, "Update installed");
}

#[test]
fn contextual_update_accepts_a_passcode_and_rereads_the_game_on_back() {
    let temp = tempfile::tempdir().unwrap();
    let base = crate::common::base_game(temp.path());
    let path = temp.path().join("locked.pkg");
    fs::write(&path, features::passcoded_pkg(2)).unwrap();
    let mut app = OrbisApp::default();
    app.open_package(path, UpdateBase::Game(base.clone()));
    finish(&mut app);
    assert!(app.preview.is_none() && app.action == Action::Update);
    assert!(matches!(
        app.package.as_ref().unwrap().update.readiness,
        UpdateReadiness::NeedsPasscode(_)
    ));
    app.passcode_input = String::from_utf8(features::PASSCODE.to_vec())
        .unwrap()
        .into();
    click(&mut app, "Validate passcode");
    finish(&mut app);
    assert!(app.preview.is_some());
    click(&mut app, "Back to game");
    finish(&mut app);
    assert_eq!(app.game.as_ref().unwrap().base, base);
    assert!(app.package.is_none() && app.passcode.is_none() && app.preview.is_none());
}

#[test]
fn interrupted_merge_offers_recovery_and_refreshes_the_restored_base() {
    let temp = tempfile::tempdir().unwrap();
    let base = crate::common::base_game(temp.path());
    let pkg = orbis_pkg::Pkg::new(crate::common::update("01.12", "new.txt", b"PATCH")).unwrap();
    orbis_cli::install::install_with_mode(
        &pkg,
        &base,
        InstallMode::Merge,
        orbis_cli::SilentProgress,
    )
    .unwrap();
    let mut app = OrbisApp::default();
    app.open(base.clone());
    finish(&mut app);
    let view = frame(&mut app, Default::default());
    assert!(
        view.nodes
            .iter()
            .any(|(_, node)| node.label() == Some("Restore base"))
    );
    assert!(
        !view
            .nodes
            .iter()
            .any(|(_, node)| matches!(node.label(), Some("Export this game…" | "Install update…")))
    );

    fs::write(
        temp.path()
            .join(".CUSA00000.orbis-install/merge/pending.json"),
        br#""restore""#,
    )
    .unwrap();
    click(&mut app, "Refresh");
    finish(&mut app);
    click(&mut app, "Recover base");
    finish(&mut app);
    assert_eq!(app.notice.as_ref().unwrap().title, "Recovery complete");
    click(&mut app, "Done");
    finish(&mut app);
    let view = frame(&mut app, Default::default());
    assert!(
        view.nodes
            .iter()
            .any(|(_, node)| node.value() == Some("Base game only"))
    );
    assert!(!base.join("new.txt").exists());
}

#[test]
fn separate_rollback_confirms_the_versions_and_refreshes_the_game_folder() {
    use orbis_cli::{SilentProgress, install};

    for (versions, button, question, expected) in [
        (
            &["01.10"][..],
            "Restore base 01.00…",
            "Switch from 01.10 to base game 01.00?",
            None,
        ),
        (
            &["01.10", "01.12"][..],
            "Restore previous update 01.10…",
            "Switch from 01.12 to previous update 01.10?",
            Some("01.10"),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::common::base_game(temp.path());
        for version in versions {
            let pkg = orbis_pkg::Pkg::new(crate::common::update(
                version,
                "base.txt",
                version.as_bytes(),
            ))
            .unwrap();
            install::install(&pkg, &base, SilentProgress).unwrap();
        }
        let active_file = temp.path().join("CUSA00000-UPDATE/base.txt");
        let mut app = OrbisApp::default();
        app.open(base.clone());
        finish(&mut app);
        click(&mut app, button);
        let view = frame(&mut app, Default::default());
        assert!(
            view.nodes
                .iter()
                .any(|(_, node)| node.value() == Some(question))
        );
        click(&mut app, "Cancel");
        assert!(app.job.is_none());
        assert_eq!(
            fs::read(&active_file).unwrap(),
            versions.last().unwrap().as_bytes()
        );

        click(&mut app, button);
        click(&mut app, "Confirm restore");
        finish(&mut app);
        assert_eq!(app.notice.as_ref().unwrap().title, "Rollback complete");
        click(&mut app, "Done");
        finish(&mut app);
        let report = app.game.as_ref().unwrap().status.as_ref().unwrap();
        assert_eq!(report.active_version.as_deref(), expected);
        assert!(report.rollback_version.is_none());
        let view = frame(&mut app, Default::default());
        assert!(!view.nodes.iter().any(|(_, node)| {
            node.label()
                .is_some_and(|label| label.starts_with("Restore "))
        }));
        assert_eq!(fs::read(base.join("base.txt")).unwrap(), b"BASE");
        match expected {
            Some(version) => assert_eq!(fs::read(active_file).unwrap(), version.as_bytes()),
            None => assert!(!active_file.parent().unwrap().exists()),
        }
    }
}
