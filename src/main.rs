use anyhow::{anyhow, bail, Error};
use inflector::cases::titlecase::to_title_case;
use log::{debug, info};
use serde_derive::Deserialize;
use std::{
    collections::HashMap,
    env,
    fs::{self},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread, time,
};
use structopt::StructOpt;
use zip::{write::FileOptions, CompressionMethod};
use zip_extensions::zip_create_from_directory_with_options;

use anyhow::Context;

#[cfg(target_os = "linux")]
use walkdir::WalkDir;

mod config;

#[cfg(target_os = "macos")]
const GCC_PATH_STR: &'static str = "/usr/local/bin/arm-none-eabi-gcc";
#[cfg(all(unix, not(target_os = "macos")))]
const GCC_PATH_STR: &'static str = "arm-none-eabi-gcc";
#[cfg(windows)]
const GCC_PATH_STR: &'static str = "arm-none-eabi-gcc.exe";

#[cfg(unix)]
#[allow(unused)]
const PDUTIL_NAME: &'static str = "pdutil";
#[cfg(windows)]
const PDUTIL_NAME: &'static str = "PDUTIL.EXE";

#[cfg(unix)]
const PDC_NAME: &'static str = "pdc";
#[cfg(windows)]
const PDC_NAME: &'static str = "PDC.EXE";

#[cfg(unix)]
const SDK_DIR: &'static str = "Developer";
#[cfg(windows)]
const SDK_DIR: &'static str = "Documents";

fn playdate_sdk_cfg() -> Result<config::SdkCfg, Error> {
    let cfg_path = dirs::home_dir()
        .ok_or(anyhow!("Can't find home dir"))?
        .join(config::CFG_DIR)
        .join(config::CFG_FILENAME);
    fs::read_to_string(cfg_path)?.parse()
}

fn playdate_sdk_path() -> Result<PathBuf, Error> {
    match playdate_sdk_cfg() {
        Err(_) => {
            debug!("Unable to read PlaydateSDK config from home dir, so using default.");
            playdate_sdk_path_default()
        }
        Ok(cfg) => cfg.sdk_path().map(|p| Ok(p)).unwrap_or_else(|| {
            debug!("Unable to determine PlaydateSDK path by config, so using default.");
            playdate_sdk_path_default()
        }),
    }
}

fn playdate_sdk_path_default() -> Result<PathBuf, Error> {
    let sdk_location = match env::var("PLAYDATE_SDK_PATH") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            // couldn't find the expected env variable, try defaulting to their home directory
            let home_dir = dirs::home_dir().ok_or(anyhow!("Can't find home dir"))?;
            home_dir.join(SDK_DIR).join("PlaydateSDK")
        }
    };
    Ok(sdk_location)
}

fn playdate_c_api_path() -> Result<PathBuf, Error> {
    Ok(playdate_sdk_path()?.join("C_API"))
}

type Assets = Vec<String>;

#[derive(Clone, Debug, Default, Deserialize)]
struct Metadata {
    name: Option<String>,
    author: Option<String>,
    description: Option<String>,
    bundle_id: Option<String>,
    version: Option<String>,
    build_number: Option<u64>,
    image_path: Option<String>,
    launch_sound_path: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Target {
    name: String,
    assets: Option<Assets>,
    metadata: Option<Metadata>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Manifest {
    #[serde(default, alias = "target")]
    targets: Vec<Target>,
}

impl Manifest {
    fn get_target(&self, target_name: &str) -> Option<&Target> {
        self.targets
            .iter()
            .find(|target| &target.name == target_name)
    }
}

pub fn load_manifest(manifest_path: &Option<PathBuf>) -> Result<Manifest, Error> {
    let cwd: PathBuf = if let Some(actual_manifest_path) = manifest_path.as_ref() {
        actual_manifest_path
            .parent()
            .expect("manifest_path parent")
            .to_path_buf()
    } else {
        std::env::current_dir()?
    };
    let manifest_path = cwd.join("Crank.toml");
    if !manifest_path.exists() {
        return Ok(Manifest::default());
    }
    let manifest_contents = fs::read_to_string(manifest_path)?;
    let manifest = toml::from_str(&manifest_contents)?;
    Ok(manifest)
}

#[derive(Debug, StructOpt)]
#[structopt(about = "Crank commands")]
enum CrankCommand {
    /// Build binary targeting Playdate device or Simulator
    Build(Build),
    /// Build binary targeting Playdate device or Simulator and run it
    Run(Build),
    /// Make a pdx file for both device and simulator and compress it.
    Package(Package),
}

#[derive(Debug, StructOpt, Clone)]
struct Build {
    /// Build for the Playdate device.
    #[structopt(long)]
    device: bool,

    /// Build artifacts in release mode, with optimizations.
    #[structopt(long)]
    release: bool,

    /// Enable build feature flags.
    #[structopt(long)]
    features: Vec<String>,

    /// Build a specific example from the examples/ dir.
    #[structopt(long)]
    example: Option<String>,

    /// Run.
    #[structopt(long)]
    run: bool,
}

impl Build {
    fn setup_path() -> Result<PathBuf, Error> {
        let playdate_c_api_path = playdate_c_api_path()?;
        Ok(playdate_c_api_path.join("buildsupport").join("setup.c"))
    }

    fn get_target_name(&self, opt: &Opt) -> Result<Option<String>, Error> {
        let mut cmd = cargo_metadata::MetadataCommand::new();
        if let Some(manifest_path) = &opt.manifest_path {
            cmd.manifest_path(manifest_path);
        }
        cmd.no_deps();
        let static_lib: String = "staticlib".to_string();
        let cdylib: String = "cdylib".to_string();
        let metadata = cmd.exec()?;
        for package in metadata.packages {
            if let Some(lib_target) = package
                .targets
                .iter()
                .filter(|target| target.kind.contains(&static_lib) && target.kind.contains(&cdylib))
                .nth(0)
            {
                return Ok(Some(lib_target.name.clone()));
            }
        }
        Ok(None)
    }

    fn compile_setup(&self, target_dir: &PathBuf) -> Result<(), Error> {
        let gcc_compile_static_args = "-g3 -c -mthumb -mcpu=cortex-m7 -mfloat-abi=hard \
        -mfpu=fpv5-sp-d16 -D__FPU_USED=1 -O2 -falign-functions=16 -fomit-frame-pointer \
        -gdwarf-2 -Wall -Wno-unused -Wstrict-prototypes -Wno-unknown-pragmas -fverbose-asm \
        -Wdouble-promotion -mword-relocations -fno-common \
        -ffunction-sections -fdata-sections -DTARGET_PLAYDATE=1 -DTARGET_EXTENSION=1 -fno-exceptions";
        let args_iter = gcc_compile_static_args.split(" ");
        let playdate_c_api_path = playdate_c_api_path()?;
        let setup_path = Self::setup_path()?;
        let mut command = Command::new(GCC_PATH_STR);
        command
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .args(args_iter)
            .arg(setup_path)
            .arg("-I")
            .arg(playdate_c_api_path)
            .arg("-o")
            .arg(target_dir.join("setup.o"));
        info!("compile_setup: {:?}", command);
        let status = command.status()?;
        if !status.success() {
            bail!("gcc failed with error {:?}", status);
        }
        Ok(())
    }

    fn link_binary(
        &self,
        target_dir: &PathBuf,
        example_name: &str,
        lib_path: &PathBuf,
    ) -> Result<(), Error> {
        let gcc_link_static_args = "-nostartfiles -mthumb -mcpu=cortex-m7 -mfloat-abi=hard \
        -mfpu=fpv5-sp-d16 -D__FPU_USED=1 -Wl,--cref,--gc-sections,--no-warn-mismatch,--emit-relocs -fno-exceptions";

        let mut cmd = Command::new(GCC_PATH_STR);
        cmd.stdout(Stdio::null()).stderr(Stdio::inherit());
        let setup_obj_path = target_dir.join("setup.o");
        cmd.arg(setup_obj_path);
        cmd.arg(lib_path);

        let args_iter = gcc_link_static_args.split(" ");
        cmd.args(args_iter);

        let playdate_c_api_path = playdate_c_api_path()?;
        let link_map_path = playdate_c_api_path.join("buildsupport").join("link_map.ld");

        cmd.arg("-T");
        cmd.arg(link_map_path);

        let target_path = target_dir.join(format!("{}.elf", example_name));
        cmd.arg("-o");
        cmd.arg(target_path);

        cmd.arg("--entry");
        cmd.arg("eventHandlerShim"); // declared in setup.c

        info!("link_binary: {:?}", cmd);

        let status = cmd.status()?;
        if !status.success() {
            bail!("gcc failed with error {:?}", status);
        }

        Ok(())
    }

    fn make_binary(
        &self,
        target_dir: &PathBuf,
        example_name: &str,
        source_dir: &PathBuf,
    ) -> Result<(), Error> {
        let source_path = target_dir.join(format!("{}.elf", example_name));
        let source_dir_path = source_dir.join("pdex.elf");

        // just copy/rename, from v2.0 pdex.bin producing by pdc by pdex.elf
        fs::copy(&source_path, &source_dir_path)?;

        Ok(())
    }

    fn make_source_dir(
        &self,
        overall_target_dir: &PathBuf,
        example_title: &str,
    ) -> Result<PathBuf, Error> {
        info!("make_source_dir");
        let pdx_path = overall_target_dir.join(example_title);
        fs::create_dir_all(&pdx_path)?;

        Ok(pdx_path)
    }

    fn copy_assets(
        &self,
        target_name: &str,
        source_dir: &Path,
        crank_manifest: &Manifest,
        dest_dir: &PathBuf,
    ) -> Result<(), Error> {
        info!("copy_assets");
        let target = crank_manifest.get_target(target_name);
        if let Some(Target {
            assets: Some(assets),
            ..
        }) = target
        {
            for asset in assets {
                let src_path = source_dir.join(asset);
                let dst_path = dest_dir.join(asset);
                info!("copy {:?} to {:?}", src_path, dst_path);
                if let Some(dst_parent) = dst_path.parent() {
                    fs::create_dir_all(&dst_parent)?;
                }
                fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    fn make_manifest(
        &self,
        crank_manifest: &Manifest,
        target_name: &str,
        source_dir: &PathBuf,
    ) -> Result<(), Error> {
        info!("make_manifest");
        let target = crank_manifest.get_target(target_name);
        if let Some(Target {
            metadata: Some(metadata),
            ..
        }) = target
        {
            let pdx_info_path = source_dir.join("pdxinfo");
            let mut pdx_info = fs::File::create(&pdx_info_path)?;

            if let Some(name) = &metadata.name {
                writeln!(pdx_info, "name={}", name)?;
            }
            if let Some(author) = &metadata.author {
                writeln!(pdx_info, "author={}", author)?;
            }
            if let Some(description) = &metadata.description {
                writeln!(pdx_info, "description={}", description)?;
            }
            if let Some(bundle_id) = &metadata.bundle_id {
                writeln!(pdx_info, "bundleID={}", bundle_id)?;
            }
            if let Some(version) = &metadata.version {
                writeln!(pdx_info, "version={}", version)?;
            }
            if let Some(build_number) = &metadata.build_number {
                writeln!(pdx_info, "buildNumber={}", build_number)?;
            }
            if let Some(image_path) = &metadata.image_path {
                writeln!(pdx_info, "imagePath={}", image_path)?;
            }
            if let Some(launch_sound_path) = &metadata.launch_sound_path {
                writeln!(pdx_info, "launchSoundPath={}", launch_sound_path)?;
            }
        }
        Ok(())
    }

    fn run_pdc(&self, source_dir: &PathBuf, dest_dir: &PathBuf) -> Result<(), Error> {
        info!("run_pdc");
        let pdc_path = playdate_sdk_path()?.join("bin").join(PDC_NAME);
        let mut cmd = Command::new(pdc_path);
        cmd.arg("--strip");
        //   cmd.arg("--verbose");
        cmd.arg(source_dir);
        cmd.arg(dest_dir);

        debug!("{:?}", cmd);

        let status = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .status()
            .with_context(|| format!("Command failed: {cmd:?}"))?;
        if !status.success() {
            bail!("pdc failed with error {:?}", status);
        }

        Ok(())
    }

    #[cfg(unix)]
    fn copy_directory(src: &Path, dst: &Path) -> Result<(), Error> {
        info!("copy_directory {:?} -> {:?}", src, dst);
        for entry in fs::read_dir(src).context("Reading source game directory")? {
            let entry = entry.context("bad entry")?;
            let target_path = dst.join(entry.file_name());
            if entry.path().is_dir() {
                fs::create_dir_all(&target_path)
                    .context(format!("Creating directory {:#?} on device", target_path))?;
                Self::copy_directory(&entry.path(), &target_path)?;
            } else {
                info!("copy_file {:?} -> {:?}", entry.path(), target_path);
                fs::copy(entry.path(), target_path).context("copy file")?;
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn run_target(&self, pdx_dir: &PathBuf, example_title: &str) -> Result<(), Error> {
        info!("run_target");
        let pdutil_path = playdate_sdk_path()?.join("bin").join(PDUTIL_NAME);
        let device_path = format!("/Games/{}.pdx", example_title);
        let duration = time::Duration::from_millis(100);

        let _ = Command::new(&pdutil_path)
            .arg("install")
            .arg(pdx_dir)
            .status()?;

        thread::sleep(duration * 5);

        let _ = Command::new(&pdutil_path)
            .arg("run")
            .arg(device_path)
            .status()?;
        Ok(())
    }

    #[cfg(unix)]
    fn run_target(&self, pdx_dir: &PathBuf, example_title: &str) -> Result<(), Error> {
        info!("run_target");

        let pdutil_path = playdate_sdk_path()?.join("bin").join(PDUTIL_NAME);
        #[cfg(target_os = "macos")]
        let modem_path = PathBuf::from(
            env::var("PLAYDATE_SERIAL_DEVICE")
                .unwrap_or(String::from("/dev/cu.usbmodemPDU1_Y0005491")),
        );
        #[cfg(target_os = "linux")]
        let modem_path = PathBuf::from(
            env::var("PLAYDATE_SERIAL_DEVICE")
                // On Linux, we can use named symlinks to find the device in most cases
                .unwrap_or(find_serial_device().unwrap_or(String::from("/dev/ttyACM0"))),
        );
        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        let modem_path = PathBuf::from(
            env::var("PLAYDATE_SERIAL_DEVICE").unwrap_or(String::from("/dev/ttyACM0")),
        );
        #[cfg(target_os = "macos")]
        let data_path = PathBuf::from(
            env::var("PLAYDATE_MOUNT_POINT").unwrap_or(String::from("/Volumes/PLAYDATE")),
        );
        #[cfg(not(target_os = "macos"))]
        let data_path = PathBuf::from(env::var("PLAYDATE_MOUNT_POINT").unwrap_or(format!(
            "/run/media/{}/PLAYDATE",
            env::var("USER").expect("user")
        )));

        let duration = time::Duration::from_millis(100);
        if modem_path.exists() {
            let mut cmd = Command::new(&pdutil_path);
            cmd.arg(modem_path.clone()).arg("datadisk").arg(pdx_dir);
            info!("datadisk cmd: {:#?}", cmd);
            let _ = cmd.status()?;

            // Note: this device doesn't disappear on one Linux developer's system; is this always
            // true?  Should we instead have a maximum delay and then continue regardless?
            #[cfg(not(target_os = "linux"))]
            while modem_path.exists() {
                thread::sleep(duration);
            }
        }

        #[cfg(target_os = "linux")]
        println!("If your OS does not automatically mount your Playdate, please do so now.");

        while !data_path.exists() {
            thread::sleep(duration);
        }

        let games_dir = data_path.join("Games");

        // This prevents issues that occur when the PLAYDATE volume is mounted
        // but not all of the inner folders are available yet.
        while !games_dir.exists() {
            thread::sleep(duration);
        }

        let game_device_dir = format!("{}.pdx", example_title);
        let games_target_dir = games_dir.join(&game_device_dir);
        fs::create_dir(&games_target_dir).ok();
        Self::copy_directory(&pdx_dir, &games_target_dir)?;

        #[cfg(target_os = "macos")]
        {
            let mut cmd = Command::new("diskutil");
            cmd.arg("eject").arg(&data_path);
            info!("eject cmd: {:#?}", cmd);
            let _ = cmd.status()?;
        }

        #[cfg(not(target_os = "macos"))]
        {
            let mut cmd = Command::new("eject");
            cmd.arg(&data_path);
            info!("eject cmd: {:#?}", cmd);
            let _ = cmd.status()?;
        }

        #[cfg(target_os = "linux")]
        println!("Please press 'A' on the Playdate to exit Data Disk mode.");

        while !modem_path.exists() {
            thread::sleep(duration);
        }

        // Note: this sleep was determined by testing on one Linux system and may not be
        // consistent; is there a better marker that we're ready to call pdutil run?
        #[cfg(target_os = "linux")]
        thread::sleep(duration * 10);

        let mut cmd = Command::new(&pdutil_path);
        cmd.arg(modem_path)
            .arg("run")
            .arg(format!("/Games/{}", game_device_dir));
        info!("run cmd: {:#?}", cmd);
        let _ = cmd.status()?;

        Ok(())
    }

    fn link_dylib(
        &self,
        target_dir: &PathBuf,
        example_name: &str,
        source_dir: &PathBuf,
    ) -> Result<(), Error> {
        info!("link_dylib");

        let (lib_target_path, source_dir_path) = if cfg!(target_os = "macos") {
            let lib_target_path = target_dir.join(format!("lib{}.dylib", example_name));
            let source_dir_path = source_dir.join("pdex.dylib");
            (lib_target_path, source_dir_path)
        } else if cfg!(unix) {
            let lib_target_path = target_dir.join(format!("lib{}.so", example_name));
            let source_dir_path = source_dir.join("pdex.so");
            (lib_target_path, source_dir_path)
        } else if cfg!(windows) {
            let lib_target_path = target_dir.join(format!("{}.dll", example_name));
            let source_dir_path = source_dir.join("pdex.dll");
            (lib_target_path, source_dir_path)
        } else {
            unreachable!("platform not supported")
        };
        debug!("copy: {:?} -> {:?}", lib_target_path, source_dir_path);
        fs::copy(&lib_target_path, &source_dir_path)?;

        let pdx_bin_path = source_dir.join("pdex.bin");
        if !pdx_bin_path.exists() {
            fs::File::create(&pdx_bin_path)?;
        }

        Ok(())
    }

    fn run_simulator(&self, pdx_path: &PathBuf) -> Result<(), Error> {
        info!("run_simulator");
        #[cfg(windows)]
        let status = {
            let mut cmd = Command::new("PlaydateSimulator.exe");
            cmd.arg(&pdx_path);
            cmd.status()?
        };

        #[cfg(target_os = "macos")]
        let status = {
            let mut cmd = Command::new("open");
            cmd.arg("-a");
            cmd.arg("Playdate Simulator");
            cmd.arg(&pdx_path);
            cmd.status()?
        };

        #[cfg(all(unix, not(target_os = "macos")))]
        let status = {
            let mut cmd = Command::new("PlaydateSimulator");
            cmd.arg(&pdx_path);
            cmd.status().or_else(|_| -> Result<ExitStatus, Error> {
                info!("falling back on SDK path");
                cmd = Command::new(playdate_sdk_path()?.join("bin").join("PlaydateSimulator"));
                cmd.arg(&pdx_path);
                Ok(cmd.status()?)
            })?
        };

        if !status.success() {
            bail!("open failed with error {:?}", status);
        }

        Ok(())
    }

    pub fn execute(
        &self,
        opt: &Opt,
        crank_manifest: &Manifest,
    ) -> Result<(PathBuf, String), Error> {
        info!("building");

        let current_dir = std::env::current_dir()?;
        let manifest_path_str;
        let mut args = if self.device {
            vec!["+nightly", "build"]
        } else {
            vec!["build"]
        };

        let project_path = if let Some(manifest_path) = opt.manifest_path.as_ref() {
            args.push("--manifest-path");
            manifest_path_str = manifest_path.to_string_lossy();
            args.push(&manifest_path_str);
            manifest_path.parent().expect("parent")
        } else {
            current_dir.as_path()
        };

        let (target_name, target_path) = if let Some(example) = self.example.as_ref() {
            args.push("--example");
            args.push(example);
            (example.clone(), format!("examples/"))
        } else {
            args.push("--lib");
            if let Some(target_name) = self.get_target_name(&opt)? {
                (target_name.clone(), "".to_string())
            } else {
                bail!("Could not find compatible target");
            }
        };

        if self.release {
            args.push("--release");
        }

        let features;
        if !self.features.is_empty() {
            features = format!("--features={}", self.features.join(","));
            args.push(&features);
        }

        if self.device {
            args.push("--target");
            args.push("thumbv7em-none-eabihf");

            args.push("-Zbuild-std=core,alloc");
            args.push("-Zbuild-std-features=panic_immediate_abort");
        }

        let envs = if self.device {
            let mut map = HashMap::new();
            map.insert(
                "RUSTFLAGS",
                [
                    "-Ctarget-cpu=cortex-m7",
                    "-Ctarget-feature=-fp64", // Rev A hardware seems to not have 64-bit floating point support
                    "-Clink-args=--emit-relocs",
                    "-Crelocation-model=pic",
                    "-Cpanic=abort",
                ]
                .join(" "),
            );
            map
        } else {
            Default::default()
        };

        let mut command = Command::new("cargo");
        command.args(args);
        command.envs(envs);
        info!("build command: {:?}", command);

        let status = command.status()?;
        if !status.success() {
            bail!("cargo failed with error {:?}", status);
        }

        let overall_target_dir = project_path.join("target");
        let game_title = crank_manifest
            .get_target(&target_name)
            .and_then(|target| target.metadata.as_ref())
            .and_then(|metadata| metadata.name.clone())
            .unwrap_or(to_title_case(&target_name));
        let package_name = target_name.replace('-', "_");
        let source_path = self.make_source_dir(&overall_target_dir, &game_title)?;
        let dest_path = overall_target_dir.join(format!("{}.pdx", &game_title));
        if dest_path.exists() {
            fs::remove_dir_all(&dest_path).unwrap_or_else(|_err| ());
        }
        let mut target_dir = project_path.join("target");
        let dir_name = if self.release { "release" } else { "debug" };
        if self.device {
            target_dir = target_dir.join("thumbv7em-none-eabihf").join(dir_name);
            let lib_file = target_dir.join(format!("{}lib{}.a", target_path, package_name));
            self.compile_setup(&target_dir)?;
            self.link_binary(&target_dir, &package_name, &lib_file)?;
            self.make_binary(&target_dir, &package_name, &source_path)?;
            self.copy_assets(&target_name, &project_path, &crank_manifest, &source_path)?;
            self.make_manifest(&crank_manifest, &target_name, &source_path)?;
            self.run_pdc(&source_path, &dest_path)?;
            if self.run {
                self.run_target(&dest_path, &game_title)?;
            }
        } else {
            target_dir = target_dir.join(dir_name).join(target_path);
            self.link_dylib(&target_dir, &package_name, &source_path)?;
            self.copy_assets(&target_name, &project_path, &crank_manifest, &source_path)?;
            self.make_manifest(&crank_manifest, &target_name, &source_path)?;
            self.run_pdc(&source_path, &dest_path)?;
            if self.run {
                self.run_simulator(&dest_path)?;
            }
        }

        Ok((dest_path, game_title))
    }
}

#[cfg(target_os = "linux")]
/// Finds the canonical (resolved) path for the Playdate serial device.  If multiple Playdate devices are
/// found, warns and returns the first.  If none is found, returns None.  If any error occurs,
/// returns None.
fn find_serial_device() -> Option<String> {
    // Walk through this directory to find Playdate device filenames
    let directory = "/dev/serial/by-id";
    let filename_prefix = "usb-Panic_Inc_Playdate_PDU1-";

    let walker = WalkDir::new(directory)
        .min_depth(1)
        .max_depth(1)
        // Don't follow links (yet) because we want file_name to give us the name in this directory
        .follow_links(false)
        // If there are multiple, we let the user know and take the first; sort so it's consistent.
        // If the user wants a different one, they can set PLAYDATE_SERIAL_DEVICE.
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            e.file_name()
                .to_str()
                .map(|s| s.starts_with(filename_prefix))
                .unwrap_or(false)
        })
        .filter_map(|e| e.ok());

    // See what we found
    let mut result: Option<PathBuf> = None;
    for entry in walker {
        match result {
            // If there are multiple matches, let the user know, and return the first
            Some(ref existing) => {
                println!(
                    "Found multiple Playdate devices in {}, using first: {}",
                    directory,
                    existing.display()
                );
                break;
            }
            None => {
                result = Some(entry.into_path());
            }
        }
    }

    if let Some(path) = result {
        // Fully resolve the link, which should result in something like "/dev/ttyACM0"
        let resolved = fs::canonicalize(path).ok()?;
        // Quick check that it did what we expected
        if resolved
            .to_str()
            .map(|s| s.contains("tty"))
            .unwrap_or(false)
        {
            println!("Resolved Playdate serial device to: {}", resolved.display());
            // Other code expects String paths
            return Some(resolved.to_string_lossy().into_owned());
        } else {
            eprintln!(
                "Warning: found a device at '{}' but it's not named like we expect.  Using the default.",
                resolved.display()
            );
            return None;
        }
    }

    None
}

#[derive(Debug, StructOpt)]
struct Package {
    /// Build a specific example from the examples/ dir.
    #[structopt(long)]
    example: Option<String>,

    /// Enable build feature flags.
    #[structopt(long)]
    features: Vec<String>,

    /// clean before building
    #[structopt(long)]
    clean: bool,

    /// Reveal the resulting archive in the Finder/Exporer
    #[structopt(long)]
    reveal: bool,
}

impl Package {
    pub fn execute(&self, opt: &Opt, crank_manifest: &Manifest) -> Result<(), Error> {
        if self.clean {
            info!("cleaning");
            let manifest_path_str;
            let mut args = Vec::new();
            if let Some(manifest_path) = opt.manifest_path.as_ref() {
                args.push("--manifest-path");
                manifest_path_str = manifest_path.to_string_lossy();
                args.push(&manifest_path_str);
            };

            let status = Command::new("cargo").arg("clean").args(args).status()?;
            if !status.success() {
                bail!("cargo failed with error {:?}", status);
            }
        }
        let device_build = Build {
            device: true,
            example: self.example.clone(),
            features: self.features.clone(),
            release: true,
            run: false,
        };
        device_build.execute(opt, crank_manifest)?;

        let sim_build = Build {
            device: false,
            example: self.example.clone(),
            features: self.features.clone(),
            release: true,
            run: false,
        };

        let (target_dir, game_title) = sim_build.execute(opt, crank_manifest)?;
        let parent = target_dir.parent().expect("parent");
        let target_archive = parent.join(format!("{}.pdx.zip", game_title));
        info!("target_dir {:#?}", target_dir);
        info!("target_archive {:#?}", target_archive);
        fs::remove_dir_all(&target_archive).unwrap_or_else(|_err| ());
        let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
        zip_create_from_directory_with_options(&target_archive, &target_dir, options)?;
        #[cfg(windows)]
        if self.reveal {
            let _ = Command::new("Explorer")
                .arg(format!("/Select,{}", target_archive.to_string_lossy()))
                .status()?;
        }
        #[cfg(target_os = "macos")]
        if self.reveal {
            let _ = Command::new("open")
                .arg("-R")
                .arg(target_archive)
                .status()?;
        }
        #[cfg(target_os = "linux")]
        if self.reveal {
            let _ = Command::new("xdg-open").arg(parent).status()?;
        }
        Ok(())
    }
}

#[derive(StructOpt, Debug)]
#[structopt(name = "crank")]
struct Opt {
    #[structopt(short, long)]
    verbose: bool,

    /// Path to Cargo.toml
    #[structopt(long, global = true)]
    manifest_path: Option<PathBuf>,

    #[structopt(subcommand)]
    cmd: CrankCommand,
}

fn main() -> Result<(), Error> {
    let opt = Opt::from_args();

    if opt.verbose {
        env::set_var("RUST_LOG", "info");
    }

    pretty_env_logger::init();

    info!("starting");

    let crank_manifest = load_manifest(&opt.manifest_path)?;

    info!("manifest = {:#?}", crank_manifest);

    match &opt.cmd {
        CrankCommand::Build(build) => {
            build.execute(&opt, &crank_manifest)?;
        }
        CrankCommand::Run(build) => {
            let build_and_run = Build {
                run: true,
                ..build.clone()
            };
            build_and_run.execute(&opt, &crank_manifest)?;
        }
        CrankCommand::Package(package) => {
            package.execute(&opt, &crank_manifest)?;
        }
    }

    Ok(())
}
