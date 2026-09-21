fn main() {
    if let Err(error) = vm_rust::native_bevy_schedule_probe::run() {
        eprintln!("native Bevy schedule probe failed: {error}");
        std::process::exit(1);
    }
    println!("native Bevy schedule probe passed");
}
