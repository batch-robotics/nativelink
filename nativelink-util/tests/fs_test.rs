#![cfg(not(target_family = "windows"))]
// Because windows does permissions differently

use std::env;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;

use nativelink_error::ResultExt;
use nativelink_macro::nativelink_test;
use nativelink_util::fs::remove_dir_all;

#[nativelink_test]
async fn remove_files_with_bad_permissions() -> Result<(), Box<dyn core::error::Error>> {
    let temp_dir = env::temp_dir();
    let bad_perms_directory = temp_dir.join("bad_perms_directory");
    if fs::exists(&bad_perms_directory)? {
        remove_dir_all(&bad_perms_directory)
            .await
            .err_tip(|| format!("first remove_dir_all for {bad_perms_directory:?}"))?;
    }
    fs::create_dir(&bad_perms_directory)?;
    let bad_perms_file = bad_perms_directory.join("bad_perms_file");
    if !fs::exists(&bad_perms_file)? {
        fs::write(&bad_perms_file, "").err_tip(|| "Can't create file")?;
    }

    fs::set_permissions(&bad_perms_directory, Permissions::from_mode(0o100)) // execute owner only
        .err_tip(|| "Can't set perms on directory")?;

    fs::set_permissions(&bad_perms_file, Permissions::from_mode(0o400)) // read owner only
        .err_tip(|| "Can't set perms on file")?;

    remove_dir_all(&bad_perms_directory)
        .await
        .err_tip(|| format!("second remove_dir_all for {bad_perms_directory:?}"))?;

    assert!(!fs::exists(&bad_perms_directory)?);
    Ok(())
}

#[cfg(target_os = "linux")]
#[nativelink_test]
async fn freebind_allows_binding_unassigned_address() -> Result<(), Box<dyn core::error::Error>> {
    use std::io::ErrorKind;

    use nativelink_util::fs::set_freebind;
    use tokio::net::TcpSocket;

    let addr = "192.0.2.1:0".parse()?;

    // Without `IP_FREEBIND` the kernel refuses to bind an unassigned address.
    let err = TcpSocket::new_v4()?.bind(addr).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::AddrNotAvailable);

    // With IP_FREEBIND the same bind succeeds.
    let socket = TcpSocket::new_v4()?;
    set_freebind(&socket)?;
    socket.bind(addr)?;

    Ok(())
}

/// Regression test: `fs::read_dir` used to call
/// `Handle::current().block_on(tokio::fs::read_dir(..))` *inside*
/// `spawn_blocking`, so every call parked a blocking-pool thread while it
/// waited for a second blocking task (tokio's `fs::read_dir` is itself a
/// `spawn_blocking`). With more concurrent `read_dir` calls in flight than
/// the pool has threads — the worker uploading an output tree with thousands
/// of directories fans every subdirectory out at once — the pool held only
/// waiters and the process deadlocked. A two-thread pool and eight
/// concurrent calls reproduce it.
#[test]
fn concurrent_read_dir_does_not_exhaust_blocking_pool() -> Result<(), Box<dyn core::error::Error>> {
    use core::time::Duration;

    use futures::future::try_join_all;
    use nativelink_util::fs::read_dir;

    const CONCURRENT_CALLS: usize = 8;

    let root = env::temp_dir().join(format!("read_dir_pool_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dirs: Vec<_> = (0..CONCURRENT_CALLS)
        .map(|i| root.join(format!("d{i}")))
        .collect();
    for dir in &dirs {
        fs::create_dir_all(dir)?;
        fs::write(dir.join("file"), "")?;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(30),
            try_join_all(dirs.iter().map(read_dir)),
        )
        .await
    });
    // Never wait for blocking tasks here: in the deadlocked case they never
    // finish, and a plain drop of the runtime would hang the test instead
    // of failing it.
    runtime.shutdown_background();
    fs::remove_dir_all(&root)?;

    let handles = result
        .map_err(|_| "read_dir calls deadlocked: blocking pool exhausted by nested block_on")??;
    assert_eq!(handles.len(), CONCURRENT_CALLS);
    Ok(())
}
