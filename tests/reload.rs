use std::{fs, time::SystemTime};
use steel::steel_vm::{builtin::BuiltInModule, engine::Engine, register_fn::RegisterFn};

#[test]
fn external_changes_preserve_dirty_buffers_and_reload_clean_buffers() {
    let path = std::env::temp_dir().join(format!(
        "nova-file-watcher-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::File::create_new(&path).unwrap();

    let mut engine = Engine::new();
    let mut clock = BuiltInModule::new("test/clock");
    clock.register_fn("old-time", || SystemTime::UNIX_EPOCH);
    let watch_path = path.to_string_lossy().into_owned();
    clock.register_fn("watch-path", move || watch_path.clone());
    engine.register_module(clock);
    engine.register_steel_module(
        "helix-file-watcher.scm".into(),
        "(provide make-empty-watcher drain-event-paths! watch-file!) (define (make-empty-watcher) '(#f #f)) (define (drain-event-paths! _) '()) (define (watch-file! _ __) #f)".into(),
    );
    engine.register_steel_module(
        "helix/editor.scm".into(),
        r#"
        (require-builtin test/clock)
        (provide editor-all-documents editor-document->path editor-document-last-saved
                 editor-document-dirty? editor-document-reload set-dirty! reload-count)
        (define dirty #f)
        (define reloads 0)
        (define (set-dirty! value) (set! dirty value))
        (define (reload-count) reloads)
        (define (editor-all-documents) '(1))
        (define (editor-document->path _) (watch-path))
        (define (editor-document-last-saved _) (old-time))
        (define (editor-document-dirty? _) dirty)
        (define (editor-document-reload _) (set! reloads (+ reloads 1)))
        "#
        .into(),
    );
    engine.register_steel_module(
        "helix/misc.scm".into(),
        r#"
        (provide register-hook! set-warning! warning-count enqueue-thread-local-callback-with-delay)
        (define warnings 0)
        (define (register-hook! _ __) #f)
        (define (set-warning! _) (set! warnings (+ warnings 1)))
        (define (warning-count) warnings)
        (define (enqueue-thread-local-callback-with-delay _ f) (f))
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
        "(provide log::info!) (define (log::info! _) #f)".into(),
    );

    engine.run(include_str!("../file-watcher.scm")).unwrap();

    let program = r#"
        (require-builtin test/clock)
        (require (only-in "helix/editor.scm" set-dirty! reload-count))
        (require (only-in "helix/misc.scm" warning-count))
        (set-dirty! #t)
        (maybe-reload (watch-path))
        (assert! (= (reload-count) 0))
        (assert! (= (warning-count) 1))
        (set-dirty! #f)
        (maybe-reload (watch-path))
        (assert! (= (reload-count) 1))
        "#;

    let result = engine.run(program);
    fs::remove_file(path).unwrap();
    result.unwrap();
}
