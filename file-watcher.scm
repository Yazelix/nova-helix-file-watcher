(require "helix-file-watcher.scm")
(require "helix/editor.scm")
(require (prefix-in helix. "helix/commands.scm"))
(require "helix/misc.scm")
(require "helix/ext.scm")
(require "helix/static.scm")
(require-builtin steel/time)

(provide spawn-watcher)

(define _watcher-pair (make-empty-watcher))
(define watch-handle (list-ref _watcher-pair 0))
(define event-handle (list-ref _watcher-pair 1))

(define *open-paths* (hash))

(define (all-open-files)
  ;; Only editor-context callers update this map; closed paths are discarded.
  (set! *open-paths*
        (foldl (lambda (doc-id paths)
                 (define path (editor-document->path doc-id))
                 (define resolved
                   (and path (try-canonicalize-path path (hash-try-get *open-paths* path))))
                 (if resolved (hash-insert paths path resolved) paths))
               (hash)
               (editor-all-documents)))
  (hash-values->list *open-paths*))

(define (try-canonicalize-path x [previous #f])
  (with-handler (lambda (err)
                  (log::info! (to-string "Failed canonicalizing path: " x err))
                  #f)
                (with-handler
                 ;; Preserve the last symlink target while a save has moved it aside.
                 (lambda (_)
                   (or previous
                       (string-append (canonicalize-path (parent-name x)) "/" (file-name x))))
                 (canonicalize-path x))))

(define (path->doc-ids path)
  (filter (lambda (doc-id)
            (define doc-path (editor-document->path doc-id))
            (and doc-path
                 (equal? (try-canonicalize-path doc-path (hash-try-get *open-paths* doc-path))
                         path)))
          (editor-all-documents)))

;; reload file only if the write time isn't the same as it is for helix
(define (maybe-reload x [thunk #f])
  (for-each
   (lambda (doc-id)
     (with-handler
      (lambda (err) (log::info! (to-string "file watcher: could not reload " x ": " err)))
      (define helix-doc-last-saved (editor-document-last-saved doc-id))
      (define file-last-modified (fs-metadata-modified (file-metadata x)))
      ;; Restoring a file can move its disk timestamp backward.
      (when (and helix-doc-last-saved (not (equal? helix-doc-last-saved file-last-modified)))
        (if (editor-document-dirty? doc-id)
            (set-warning! (to-string "File changed on disk; unsaved buffer preserved: " x))
            (begin
              (log::info! (to-string "reloading file: " x))
              (editor-document-reload doc-id)
              (when thunk
                (thunk)))))))
   (path->doc-ids x)))

(define *min-poll-ms* 50)
(define *max-poll-ms* 500)

(define (poll-paths)
  (with-handler (lambda (err)
                  (log::info! (to-string "file watcher: event channel closed: " err))
                  #f)
                (drain-event-paths! event-handle)))

(define (handle-changed-paths paths delay-ms)
  (with-handler
   (lambda (err) (log::info! (to-string "file watcher: " err)))
   (define changed (filter id (map try-canonicalize-path paths)))
   (define open-buffers (hx.block-on-task (lambda () (all-open-files))))
   ;; Lots of allocation!
   (define intersection
     (hashset->list (hashset-intersection (list->hashset changed) (list->hashset open-buffers))))
   (unless (empty? intersection)
     (hx.with-context (lambda ()
                        ;; Give helix like, 5 seconds to make an edit before deciding to update
                        ;; Enqueue a callback with a delay, without blocking the thread.
                        (enqueue-thread-local-callback-with-delay
                         delay-ms
                         (lambda () (for-each maybe-reload intersection))))))))

(define (loop-events delay-ms)
  (let loop ([poll-ms *min-poll-ms*])
    (define paths (poll-paths))
    (cond
      [(not paths) (log::info! "file watcher: shutting down event loop")]
      [(empty? paths)
       (time/sleep-ms poll-ms)
       (loop (min (* 2 poll-ms) *max-poll-ms*))]
      [else
       (handle-changed-paths paths delay-ms)
       (loop *min-poll-ms*)])))

(define (set-watch-files paths)
  (with-handler
   (lambda (err)
     (log::info! (to-string "file watcher: registration failed: " err))
     (set-warning! (to-string "File watcher could not update subscriptions: " err)))
   (set-watched-files! watch-handle paths)))

(define *started* #f)

(for-each (lambda (hook)
            (register-hook! hook
                            (lambda (_)
                              (when *started*
                                (set-watch-files (all-open-files))))))
          '(document-opened document-saved document-closed))

;;@doc
;; Spawn a file watcher which will reload
(define (spawn-watcher [delay-ms 2000])
  (log::info! (to-string "setting initial watched files"))
  (set-watch-files (all-open-files))
  (define worker
    (spawn-native-thread (lambda ()
                           (log::info! "starting event loop")
                           (loop-events delay-ms))))
  (set! *started* #t)
  worker)
