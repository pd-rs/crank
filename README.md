# crank

Crank is a wrapper for cargo when creating games for the [Playdate handheld gaming system](https://play.date) in Rust. This is just a tool, the actually Rust wrappers for Playdate are found in its own [repository](https://github.com/rtsuk/crankstart).

This software is not sponsored or supported by Panic.

## Requirements

 * The Playdate SDK installed in `$HOME/Developer/PlaydateSDK` on Linux or MacOS, `$HOME/Documents/PlaydateSDK` on Window, or at the path specified by the `$PLAYDATE_SDK_PATH` environment variable.
 * Rust, easiest installed via [rustup](https://rustup.rs).
 * Switch to the nightly toolchain using `rustup toolchain install nightly`, required for the `build-std` feature.
 * If you want to build for the Playdate device, you will need the `thumbv7em-none-eabihf` target. Added with `rustup +nightly target add thumbv7em-none-eabihf`
 * All the requirements listed in [Inside Playdate with C](https://sdk.play.date/inside-playdate-with-c#_prerequisites).
     * The GCC ARM compiler must be available in your system `PATH` environment variable. (This is usually done for you by the installer).

## Installation

Since crank is not yet on crates.io, one needs to download it with git and install it with cargo.

```shell
cargo install --git=https://github.com/pd-rs/crank
```

After that one should be able to run crank

```shell
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
```

The command `build` is a bit of a misnomer, as it both builds, creates a `.pdx` directory and runs the game on the simulator or device.

In order to include assets like images, crank optionally reads a `Crank.toml` file with lists of files to include in the .pdx directory. See the wrapper repository for an example.

Crank is only regularly tested on Mac, but has worked on Windows and Linux in the past.
