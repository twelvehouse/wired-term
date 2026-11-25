//! Prints the detected shell profiles and how long detection took.
#![allow(clippy::print_stdout)] // Printing the result is the purpose of this tool.

fn main() {
    let started = std::time::Instant::now();
    let profiles = wired_platform::detect_shells();
    let elapsed = started.elapsed();
    for profile in &profiles {
        println!(
            "{:<28} {:<26} {} {:?} {:?}",
            profile.id,
            profile.name,
            profile.program.display(),
            profile.args,
            profile.env
        );
    }
    println!("{} profiles in {elapsed:?}", profiles.len());
}
