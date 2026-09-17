use vm_rust::browser_e2e_test;

browser_e2e_test!(test_browser_handle_nested_input, |player| async move {
    player.test_browser_handle_nested_input().await
});
