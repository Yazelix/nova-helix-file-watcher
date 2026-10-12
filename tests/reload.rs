use std::{fs, time::SystemTime};
use steel::steel_vm::{builtin::BuiltInModule, engine::Engine, register_fn::RegisterFn};

#[test]
#[cfg(unix)]
fn watcher_preserves_dirty_buffers_and_watches_new_documents() {
    let root = std::env::temp_dir().join(format!(
        "nova-file-watcher-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let path = root.join("victim");
    let control = root.join("control");
    let alias = root.join("alias");
    fs::write(&path, "victim").unwrap();
    fs::write(&control, "control").unwrap();
    std::os::unix::fs::symlink(&path, &alias).unwrap();

    let mut engine = Engine::new();
    let mut clock = BuiltInModule::new("test/clock");
    clock.register_fn("old-time", || SystemTime::UNIX_EPOCH);
    let watch_path = path.to_string_lossy().into_owned();
    clock.register_fn("watch-path", move || watch_path.clone());
    clock.register_fn("control-path", move || {
        control.to_string_lossy().into_owned()
    });
    clock.register_fn("alias-path", move || alias.to_string_lossy().into_owned());
    engine.register_module(clock);
    engine.register_steel_module(
        "helix-file-watcher.scm".into(),
        r#"
        (provide make-empty-watcher drain-event-paths! set-watched-files! watched-paths fail-registration!)
        (define watched '())
        (define fail? #f)
        (define (watched-paths) watched)
        (define (fail-registration! value) (set! fail? value))
        (define (make-empty-watcher) '(#f #f))
        (define (drain-event-paths! _) '())
        (define (set-watched-files! _ paths)
          (if fail? (error! "registration failed") (set! watched paths)))
        "#.into(),
    );
    engine.register_steel_module(
        "helix/editor.scm".into(),
        r#"
        (require-builtin test/clock)
        (provide editor-all-documents editor-document->path editor-document-last-saved
                 editor-document-dirty? editor-document-reload set-dirty! reload-count set-documents! set-document-path!)
        (define dirty #f)
        (define reloads 0)
        (define (set-dirty! value) (set! dirty value))
        (define (reload-count) reloads)
        (define documents '(1))
        (define (set-documents! value) (set! documents value))
        (define (editor-all-documents) documents)
        (define document-path (watch-path))
        (define (set-document-path! value) (set! document-path value))
        (define (editor-document->path doc) (if (= doc 1) document-path (control-path)))
        (define (editor-document-last-saved _) (old-time))
        (define (editor-document-dirty? _) dirty)
        (define (editor-document-reload _) (set! reloads (+ reloads 1)))
        "#
        .into(),
    );
    engine.register_steel_module(
        "helix/misc.scm".into(),
        r#"
        (provide register-hook! fire-hook! set-warning! warning-count enqueue-thread-local-callback-with-delay run-callbacks!)
        (define warnings 0)
        (define hooks (hash))
        (define (register-hook! hook callback) (set! hooks (hash-insert hooks hook callback)))
        (define (fire-hook! hook) ((hash-ref hooks hook) 1))
        (define (set-warning! _) (set! warnings (+ warnings 1)))
        (define (warning-count) warnings)
        (define pending '())
        (define (enqueue-thread-local-callback-with-delay _ f) (set! pending (cons f pending)))
        (define (run-callbacks!)
          (define callbacks pending)
          (set! pending '())
          (for-each (lambda (f) (f)) callbacks))
        "#
        .into(),
    );
    engine.register_steel_module(
        "helix/ext.scm".into(),
        "(provide hx.block-on-task hx.with-context) (define (hx.block-on-task f) (f)) (define (hx.with-context f) (f))".into(),
    );
    engine.register_steel_module(
        "helix/commands.scm".into(),
        "(provide dummy) (define dummy #f)".into(),
    );
    engine.register_steel_module(
        "helix/static.scm".into(),
        "(provide log::info! spawn-native-thread) (define (log::info! _) #f) (define (spawn-native-thread _) #f)".into(),
    );

    engine.run(include_str!("../file-watcher.scm")).unwrap();

    let program = r#"
        (require-builtin test/clock)
        (require (only-in "helix/editor.scm" set-dirty! reload-count set-documents! set-document-path!))
        (require (only-in "helix/misc.scm" fire-hook! warning-count run-callbacks!))
        (require (only-in "helix-file-watcher.scm" watched-paths fail-registration!))
        (set-dirty! #t)
        (maybe-reload (watch-path))
        (assert! (= (reload-count) 0))
        (assert! (= (warning-count) 1))
        (set-dirty! #f)
        (maybe-reload (watch-path))
        (assert! (= (reload-count) 1))
        (set-documents! '())
        (spawn-watcher)
        (assert! (empty? (watched-paths)))
        (set-documents! '(1))
        (fire-hook! 'document-opened)
        (assert! (equal? (watched-paths) (list (watch-path))))
        (set-documents! '())
        (fire-hook! 'document-closed)
        (assert! (empty? (watched-paths)))
        (set-documents! '(1))
        (fire-hook! 'document-saved)
        (assert! (equal? (watched-paths) (list (watch-path))))
        (fail-registration! #t)
        (fire-hook! 'document-opened)
        (assert! (= (warning-count) 2))
        "#;

    let result = engine.run(program);
    let alias_result = engine.run(
        r#"
        (set-document-path! (alias-path))
        (maybe-reload (watch-path))
        (unless (= (reload-count) 2) (error! "canonical event did not match the document alias"))
    "#,
    );
    fs::rename(&path, root.join("backup")).unwrap();
    let missing_result = engine.run(
        r#"
        (set-document-path! (watch-path))
        (fail-registration! #f)
        (set-documents! '(1))
        (fire-hook! 'document-closed)
        (unless (equal? (watched-paths) (list (watch-path)))
          (error! "a temporarily missing open file lost its subscription"))
    "#,
    );
    fs::rename(root.join("backup"), &path).unwrap();
    engine
        .run(
            r#"
        (set-documents! '(1 2))
        (handle-changed-paths (list (watch-path) (control-path)) 0)
    "#,
        )
        .unwrap();
    fs::remove_file(&path).unwrap();
    let callback_result = engine.run(
        r#"
        (run-callbacks!)
        (unless (= (reload-count) 3) (error! "a missing file prevented the live control from reloading"))
    "#,
    );
    fs::remove_dir_all(root).unwrap();
    let failures: Vec<_> = [
        ("reload policy and hooks", result),
        ("path alias", alias_result),
        ("temporarily missing file", missing_result),
        ("delayed callback deletion", callback_result),
    ]
    .into_iter()
    .filter_map(|(case, result)| result.err().map(|error| format!("{case}: {error}")))
    .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}
