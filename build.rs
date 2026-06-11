use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=vendor/media-control/bin/media-control");
    println!("cargo:rerun-if-changed=vendor/media-control/mediaremote-adapter/bin/mediaremote-adapter.pl");
    println!("cargo:rerun-if-changed=vendor/media-control/mediaremote-adapter/include");
    println!("cargo:rerun-if-changed=vendor/media-control/mediaremote-adapter/src");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source_dir = manifest_dir.join("vendor/media-control");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("out dir"));
    let install_dir = out_dir.join("media-control");

    if build_media_control(&source_dir, &install_dir).is_err() {
        println!(
            "cargo:warning=failed to build bundled media-control; now playing helper will use PATH fallback"
        );
        return;
    }

    let helper = install_dir.join("bin/media-control");
    println!(
        "cargo:rustc-env=SOUNDBAR_BUNDLED_MEDIA_CONTROL={}",
        helper.display()
    );
}

fn build_media_control(source_dir: &Path, install_dir: &Path) -> Result<(), ()> {
    let adapter_dir = source_dir.join("mediaremote-adapter");
    let bin_dir = install_dir.join("bin");
    let lib_dir = install_dir.join("lib/media-control");
    let frameworks_dir = install_dir.join("Frameworks");
    let framework_dir = frameworks_dir.join("MediaRemoteAdapter.framework");

    fs::create_dir_all(&bin_dir).map_err(|_| ())?;
    fs::create_dir_all(&lib_dir).map_err(|_| ())?;
    fs::create_dir_all(&framework_dir).map_err(|_| ())?;

    copy_executable(
        &source_dir.join("bin/media-control"),
        &bin_dir.join("media-control"),
    )?;
    copy_executable(
        &adapter_dir.join("bin/mediaremote-adapter.pl"),
        &lib_dir.join("mediaremote-adapter.pl"),
    )?;

    compile_adapter_framework(
        &adapter_dir,
        &framework_dir.join("MediaRemoteAdapter"),
    )?;
    compile_test_client(
        &adapter_dir,
        &lib_dir.join("MediaRemoteAdapterTestClient"),
    )?;
    ad_hoc_codesign(&framework_dir);

    Ok(())
}

fn compile_adapter_framework(adapter_dir: &Path, output: &Path) -> Result<(), ()> {
    let sources = [
        "src/adapter/env.m",
        "src/adapter/get.m",
        "src/adapter/globals.m",
        "src/adapter/keys.m",
        "src/adapter/now_playing.m",
        "src/adapter/repeat.m",
        "src/adapter/seek.m",
        "src/adapter/send.m",
        "src/adapter/shuffle.m",
        "src/adapter/speed.m",
        "src/adapter/stream.m",
        "src/adapter/test.m",
        "src/private/MediaRemote.m",
        "src/utility/Debounce.m",
        "src/utility/helpers.m",
    ];

    let mut command = compiler();
    command
        .arg("-dynamiclib")
        .arg("-fobjc-arc")
        .arg("-fvisibility=default")
        .arg("-install_name")
        .arg("@rpath/MediaRemoteAdapter.framework/MediaRemoteAdapter")
        .arg("-I")
        .arg(adapter_dir.join("include"))
        .arg("-I")
        .arg(adapter_dir.join("src"));

    for source in sources {
        command.arg(adapter_dir.join(source));
    }

    command
        .arg("-framework")
        .arg("Foundation")
        .arg("-framework")
        .arg("AppKit")
        .arg("-framework")
        .arg("UniformTypeIdentifiers")
        .arg("-o")
        .arg(output);

    run(&mut command)
}

fn compile_test_client(adapter_dir: &Path, output: &Path) -> Result<(), ()> {
    let mut command = compiler();
    command
        .arg("-fobjc-arc")
        .arg("-I")
        .arg(adapter_dir.join("src/test"))
        .arg(adapter_dir.join("src/test/main.m"))
        .arg(adapter_dir.join("src/test/NowPlayingTest.m"))
        .arg("-framework")
        .arg("Foundation")
        .arg("-framework")
        .arg("MediaPlayer")
        .arg("-o")
        .arg(output);

    run(&mut command)?;
    make_executable(output)
}

fn copy_executable(source: &Path, destination: &Path) -> Result<(), ()> {
    fs::copy(source, destination).map_err(|_| ())?;
    make_executable(destination)
}

fn make_executable(path: &Path) -> Result<(), ()> {
    let mut permissions = fs::metadata(path).map_err(|_| ())?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|_| ())
}

fn ad_hoc_codesign(path: &Path) {
    let _ = Command::new("codesign")
        .arg("--force")
        .arg("--deep")
        .arg("--sign")
        .arg("-")
        .arg(path)
        .status();
}

fn compiler() -> Command {
    Command::new(env::var_os("CC").unwrap_or_else(|| "cc".into()))
}

fn run(command: &mut Command) -> Result<(), ()> {
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        _ => Err(()),
    }
}
