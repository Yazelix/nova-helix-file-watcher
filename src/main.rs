use nova_helix_file_watcher::build_module;

fn main() {
    build_module()
        .emit_package_to_file("libnova_helix_file_watcher", "helix-file-watcher.scm")
        .unwrap()
}
