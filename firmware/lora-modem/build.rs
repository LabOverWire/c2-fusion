fn main() {
    linker_be_nice();
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if let [_, kind, what, ..] = args.as_slice() {
        if kind == "undefined-symbol" {
            let hint = match what.as_str() {
                "_stack_start" => Some("Is the linker script `linkall.x` missing?"),
                what if what.starts_with("esp_rtos_") => Some(
                    "`esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler.",
                ),
                "free" | "malloc" | "calloc" | "get_free_internal_heap_size" | "malloc_internal"
                | "realloc_internal" | "calloc_internal" | "free_internal" => Some(
                    "Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?",
                ),
                _ => None,
            };
            if let Some(hint) = hint {
                eprintln!();
                eprintln!("{hint}");
                eprintln!();
            }
            std::process::exit(0);
        }
        std::process::exit(1);
    }

    if let Ok(exe) = std::env::current_exe() {
        println!(
            "cargo:rustc-link-arg=--error-handling-script={}",
            exe.display()
        );
    }
}
