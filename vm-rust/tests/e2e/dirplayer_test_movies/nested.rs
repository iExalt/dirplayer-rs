use vm_rust::browser_e2e_test;

browser_e2e_test!(test_browser_handle_nested_input, |player| async move {
    player.test_browser_handle_nested_input().await
});

browser_e2e_test!(test_browser_handle_public_play, |player| async move {
    player.test_browser_handle_public_play().await
});

browser_e2e_test!(test_browser_handle_flash_scripted_access_owner_capabilities, |player| async move {
    player.test_flash_scripted_access_owner_capabilities().await
});

browser_e2e_test!(test_flash_owned_evaluator_binding, |player| async move {
    player.test_flash_owned_evaluator_binding().await
});
