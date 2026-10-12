use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        mpsc::{channel, Receiver, TryRecvError},
        Arc, Mutex,
    },
};

use abi_stable::std_types::{RBoxError, RResult, RString, RVec};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use steel::{
    rvals::Custom,
    steel_vm::ffi::{FFIModule, FFIValue, IntoFFIVal, RegisterFFIFn},
};

steel::declare_module!(build_module);

fn file_watcher_module() -> FFIModule {
    let mut module = FFIModule::new("steel/nova-file-watcher");

    module
        .register_fn("watch-files", watch_files)
        .register_fn("try-receive-event!", EventHandle::try_recv)
        .register_fn("drain-event-paths!", EventHandle::drain_paths)
        .register_fn("event-paths", NotifyEvent::paths)
        .register_fn("event-kind", NotifyEvent::kind)
        .register_fn("make-empty-watcher", spawn_empty_watcher)
        .register_fn("set-watched-files!", WatchHandle::set_files)
        .register_fn("watch-file!", WatchHandle::watch_file)
        .register_fn("unwatch-file!", WatchHandle::unwatch_file);

    module
}

struct WatchHandle(Mutex<WatchedFiles>);
struct WatchedFiles {
    watcher: RecommendedWatcher,
    paths: Arc<Mutex<HashSet<PathBuf>>>,
    directories: HashSet<PathBuf>,
}
struct EventHandle(Mutex<Receiver<Event>>);
struct NotifyEvent(Event);

impl Custom for WatchHandle {}
impl Custom for EventHandle {}
impl Custom for NotifyEvent {}

impl WatchHandle {
    fn set_files(&self, paths: Vec<String>) -> RResult<FFIValue, RBoxError> {
        self.0
            .lock()
            .unwrap()
            .set_files(paths.into_iter().map(PathBuf::from).collect())
            .map(|()| FFIValue::Void)
            .map_err(RBoxError::new)
            .into()
    }

    fn watch_file(&self, path: String) -> RResult<FFIValue, RBoxError> {
        let mut state = self.0.lock().unwrap();
        let mut paths = state.paths.lock().unwrap().clone();
        paths.insert(PathBuf::from(path));
        state
            .set_files(paths)
            .map(|()| FFIValue::Void)
            .map_err(RBoxError::new)
            .into()
    }

    fn unwatch_file(&self, path: String) -> RResult<FFIValue, RBoxError> {
        let mut state = self.0.lock().unwrap();
        let mut paths = state.paths.lock().unwrap().clone();
        let path = match std::path::absolute(path) {
            Ok(path) => path,
            Err(error) => return RResult::RErr(RBoxError::new(error)),
        };
        let result = if paths.remove(&path) {
            state.set_files(paths)
        } else {
            // Preserve unwatching a recursive root created by watch-files.
            state.watcher.unwatch(&path)
        };
        result
            .map(|()| FFIValue::Void)
            .map_err(RBoxError::new)
            .into()
    }
}

impl WatchedFiles {
    fn set_files(&mut self, paths: HashSet<PathBuf>) -> notify::Result<()> {
        let paths: HashSet<_> = paths
            .into_iter()
            .map(std::path::absolute)
            .collect::<std::io::Result<_>>()?;
        let parents: HashSet<_> = paths
            .iter()
            .filter_map(|path| path.parent().map(PathBuf::from))
            .collect();
        for parent in parents
            .difference(&self.directories)
            .cloned()
            .collect::<Vec<_>>()
        {
            // File watches follow an inode and disappear after atomic saves.
            self.watcher.watch(&parent, RecursiveMode::NonRecursive)?;
            self.directories.insert(parent);
        }
        *self.paths.lock().unwrap() = paths;
        for parent in self
            .directories
            .difference(&parents)
            .cloned()
            .collect::<Vec<_>>()
        {
            self.watcher.unwatch(&parent)?;
            self.directories.remove(&parent);
        }
        Ok(())
    }
}

impl EventHandle {
    fn try_recv(&self) -> RResult<FFIValue, RBoxError> {
        match self.0.lock().unwrap().try_recv() {
            Ok(event) => RResult::ROk(NotifyEvent(event).into_ffi_val().unwrap()),
            Err(TryRecvError::Empty) => RResult::ROk(FFIValue::BoolV(false)),
            Err(err @ TryRecvError::Disconnected) => RResult::RErr(RBoxError::new(err)),
        }
    }

    fn drain_paths(&self) -> RResult<FFIValue, RBoxError> {
        let receiver = self.0.lock().unwrap();

        let mut seen = HashSet::new();
        let mut paths = RVec::new();

        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    for path in event.paths {
                        if let Some(path) = path.as_os_str().to_str() {
                            if seen.insert(path.to_owned()) {
                                paths.push(FFIValue::StringV(RString::from(path)));
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(err @ TryRecvError::Disconnected) => {
                    if paths.is_empty() {
                        return RResult::RErr(RBoxError::new(err));
                    }

                    break;
                }
            }
        }

        RResult::ROk(FFIValue::Vector(paths))
    }
}

impl NotifyEvent {
    pub fn kind(&self) -> FFIValue {
        match self.0.kind {
            // notify::EventKind::Any => todo!(),
            // notify::EventKind::Access(access_kind) => todo!(),
            // notify::EventKind::Create(create_kind) => todo!(),
            notify::EventKind::Modify(_) => FFIValue::StringV(RString::from("modified")),
            // notify::EventKind::Remove(remove_kind) => todo!(),
            // notify::EventKind::Other => todo!(),
            _ => FFIValue::BoolV(false),
        }
    }

    pub fn paths(&self) -> RVec<FFIValue> {
        self.0
            .paths
            .iter()
            .map(|x| FFIValue::StringV(RString::from(x.as_os_str().to_str().unwrap())))
            .collect()
    }
}

fn watcher_pair(
    initial: Option<(PathBuf, RecursiveMode)>,
) -> notify::Result<(WatchHandle, EventHandle)> {
    let initial = initial
        .map(|(path, mode)| std::path::absolute(path).map(|path| (path, mode)))
        .transpose()?;
    let (event_tx, event_rx) = channel::<Event>();
    let paths = Arc::new(Mutex::new(HashSet::new()));
    let watched_paths = Arc::clone(&paths);
    let recursive_root = initial.as_ref().map(|(path, _)| path.clone());

    let mut watcher = notify::recommended_watcher(move |event: Result<Event, _>| {
        if let Ok(mut event) = event {
            if !matches!(event.kind, notify::EventKind::Access(_)) {
                let paths = watched_paths.lock().unwrap();
                if event.need_rescan() {
                    event.paths.extend(paths.iter().cloned());
                }
                event.paths.retain(|path| {
                    paths.contains(path)
                        || recursive_root
                            .as_ref()
                            .is_some_and(|root| path.starts_with(root))
                });
                if event.paths.is_empty() {
                    return;
                }
                let _ = event_tx.send(event);
            }
        }
    })?;

    if let Some((path, mode)) = initial {
        watcher.watch(&path, mode)?;
    }

    let watch_handle = WatchHandle(Mutex::new(WatchedFiles {
        watcher,
        paths,
        directories: HashSet::new(),
    }));
    let event_handle = EventHandle(Mutex::new(event_rx));
    Ok((watch_handle, event_handle))
}

fn make_pair(initial: Option<(PathBuf, RecursiveMode)>) -> RResult<FFIValue, RBoxError> {
    let (watch_handle, event_handle) = match watcher_pair(initial) {
        Ok(pair) => pair,
        Err(error) => return RResult::RErr(RBoxError::new(error)),
    };
    let v: RVec<FFIValue> = [
        watch_handle
            .into_ffi_val()
            .expect("WatchHandle into_ffi_val"),
        event_handle
            .into_ffi_val()
            .expect("EventHandle into_ffi_val"),
    ]
    .into_iter()
    .collect();
    RResult::ROk(FFIValue::Vector(v))
}

fn spawn_empty_watcher() -> RResult<FFIValue, RBoxError> {
    make_pair(None)
}

fn watch_files(path: String) -> RResult<FFIValue, RBoxError> {
    make_pair(Some((PathBuf::from(path), RecursiveMode::Recursive)))
}

pub fn build_module() -> FFIModule {
    file_watcher_module()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    fn drain(events: &EventHandle) {
        while events
            .0
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_millis(200))
            .is_ok()
        {}
    }

    fn receive(events: &EventHandle, path: &PathBuf) {
        let event = events
            .0
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .expect("file event missing");
        assert_eq!(event.paths, [path.clone()]);
    }

    #[test]
    fn repeated_replacements_keep_delivering_file_events() {
        let root = std::env::temp_dir().join(format!(
            "nova-watcher-native-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let victim = root.join("victim");
        let control = root.join("control");
        fs::write(&victim, "initial").unwrap();
        fs::write(&control, "control").unwrap();
        let (watcher, events) = watcher_pair(None).unwrap();
        watcher
            .set_files(vec![
                victim.to_str().unwrap().to_owned(),
                control.to_str().unwrap().to_owned(),
            ])
            .unwrap();
        for round in 0..3 {
            let replacement = root.join("replacement");
            if round == 1 {
                // Helix moves the old inode aside before creating its replacement.
                fs::rename(&victim, &replacement).unwrap();
                fs::write(&victim, "editor save").unwrap();
                fs::remove_file(&replacement).unwrap();
            } else {
                fs::write(&replacement, "replaced").unwrap();
                fs::rename(&replacement, &victim).unwrap();
            }
            receive(&events, &victim);
            drain(&events);
            fs::write(&victim, "later write").unwrap();
            receive(&events, &victim);
            drain(&events);
        }

        watcher
            .unwatch_file(victim.to_str().unwrap().to_owned())
            .unwrap();
        fs::write(&victim, "closed document").unwrap();
        fs::write(root.join("unrelated"), "sibling activity").unwrap();
        assert!(events
            .0
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_millis(300))
            .is_err());
        fs::write(&control, "still watched").unwrap();
        receive(&events, &control);
        drain(&events);
        watcher.set_files(vec![]).unwrap();
        assert!(
            watcher.0.lock().unwrap().directories.is_empty(),
            "last close leaked its directory watch"
        );
        assert!(watcher.watch_file(String::new()).is_err());
        assert!(watcher
            .watch_file(root.join("missing/file").to_str().unwrap().to_owned())
            .is_err());
        fs::create_dir(root.join("missing")).unwrap();
        watcher
            .watch_file(root.join("missing/file").to_str().unwrap().to_owned())
            .unwrap();
        fs::write(root.join("missing/file"), "recovered").unwrap();
        receive(&events, &root.join("missing/file"));
        drop(watcher);
        fs::remove_dir_all(root).unwrap();
    }
}
