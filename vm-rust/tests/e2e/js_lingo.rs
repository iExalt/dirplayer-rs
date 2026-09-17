use vm_rust::browser_e2e_test;

browser_e2e_test!(test_js_object_owner_bridge, |player| async move {
    player.test_js_object_owner_bridge().await
});
