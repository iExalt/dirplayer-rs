use vm_rust::browser_e2e_test;

browser_e2e_test!(test_fileio_open_remote, |player| async move {
    player.test_fileio_open_remote().await
});
