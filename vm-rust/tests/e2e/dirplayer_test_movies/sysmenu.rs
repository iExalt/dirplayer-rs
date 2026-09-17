use vm_rust::browser_e2e_test;

/// Exercises SysMenu Print/MessageBox through the real global-dispatch and
/// browser host boundary. The alert hook re-enters reset to prove stale
/// completions are rejected after the owner rotates.
browser_e2e_test!(test_sysmenu_host_effects, |player| async move {
    player.test_sysmenu_host_effects().await
});
