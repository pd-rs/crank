# crank

Crank is a wrapper for cargo when creating games for the [Playdate handheld gaming system](https://play.date). This is just a tool, the actually Rust wrappers for Playdate are found in its own [repository](https://github.com/rtsuk/crankstart).

This software is not sponsored or supported by Panic.

## Requirements

The Playdate SDK installed in `$HOME/Developer/PlaydateSDK`.

Rust, easiest installed via [rustup](https://rustup.rs)

[cargo-xbuild](https://github.com/rust-osdev/cargo-xbuild), installed with `cargo install cargo-xbuild`, if you want to build for the Playdate device rather than the simulator.

## Installation

Since crank is not yet on crates.io, one needs to download it with git and install it with cargo.

    git clone https://github.com/rtsuk/crank.git
    cd crank
    cargo install --path . --force

After that one should be able to run crank

    crankstart $ crank build -h
    crank-build 0.1.0
    Build binary targeting Playdate device or Simulator

    USAGE:
        crank build [FLAGS] [OPTIONS]

    FLAGS:
            --device     Build for the Playdate device
        -h, --help       Prints help information
            --release    Build artifacts in release mode, with optimizations
            --run        Run
        -V, --version    Prints version information

    OPTIONS:
            --example <example>                Build a specific example from the examples/ dir
            --manifest-path <manifest-path>    Path to Cargo.toml

The command `build` is a bit of a misnomer, as it both builds, creates a .pdx directory and runs the game on the simulator or device.

In order to include assets like images, crank optionally reads a Crank.toml file with lists of files to include in the .pdx directory. See the wrapper repository for an example.

Crank is only regularly tested on Mac, but has worked on Windows in the past.