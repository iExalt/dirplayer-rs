use vm_rust::browser_e2e_test;

/// Exercises BudAPI MessageBox through the real global-dispatch and browser
/// host boundary. The mocked alert re-enters reset so a late completion cannot
/// mutate the replacement owner.
browser_e2e_test!(test_budapi_host_effects, |player| async move {
    player.test_budapi_host_effects().await
});
