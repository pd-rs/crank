use anyhow::{anyhow, bail, Error};
use inflector::cases::titlecase::to_title_case;
use log::{debug, info};
use serde_derive::Deserialize;
use std::{
    env,
    fs::{self},
    path::{Path, PathBuf},
    process::Command,
    thread, time,
};
use structopt::StructOpt;
use zip::{write::FileOptions, CompressionMethod};
use zip_extensions::zip_create_from_directory_with_options;

#[cfg(unix)]
use anyhow::Context;

mod config;

#[cfg(unix)]
const GCC_PATH_STR: &'static str = "/usr/local/bin/arm-none-eabi-gcc";
#[cfg(windows)]
const GCC_PATH_STR: &'static str =
    r"C:\Program Files (x86)\GNU Tools Arm Embedded\9 2019-q4-major\bin\arm-none-eabi-gcc.exe";

#[cfg(unix)]
const OBJCOPY_PATH_STR: &'static str = "/usr/local/bin/arm-none-eabi-objcopy";
#[cfg(windows)]
const OBJCOPY_PATH_STR: &'static str =
    r"C:\Program Files (x86)\GNU Tools Arm Embedded\9 2019-q4-major\bin\arm-none-eabi-objcopy.exe";

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
    let home_dir = dirs::home_dir().ok_or(anyhow!("Can't find home dir"))?;
    Ok(home_dir.join(SDK_DIR).join("PlaydateSDK"))
}

fn playdate_c_api_path() -> Result<PathBuf, Error> {
    Ok(playdate_sdk_path()?.join("C_API"))
}

type Assets = Vec<String>;

#[derive(Clone, Debug, Default, Deserialize)]
struct Target {
    name: String,
    assets: Assets,
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
        let gcc_compile_static_args = "-g -c -mthumb -mcpu=cortex-m7 -mfloat-abi=hard \
        -mfpu=fpv4-sp-d16 -D__FPU_USED=1 -O2 -falign-functions=16 -fomit-frame-pointer \
        -gdwarf-2 -Wall -Wno-unused -Wstrict-prototypes -Wno-unknown-pragmas -fverbose-asm \
        -ffunction-sections -fdata-sections -DTARGET_PLAYDATE=1 -DTARGET_EXTENSION=1";
        let args_iter = gcc_compile_static_args.split(" ");
        let playdate_c_api_path = playdate_c_api_path()?;
        let setup_path = Self::setup_path()?;
        let mut command = Command::new(GCC_PATH_STR);
        command
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
        let gcc_link_static_args = "-mthumb -mcpu=cortex-m7 -mfloat-abi=hard \
        -mfpu=fpv4-sp-d16 -D__FPU_USED=1 -Wl,--gc-sections,--no-warn-mismatch";

        let mut cmd = Command::new(GCC_PATH_STR);
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
        let mut cmd = Command::new(OBJCOPY_PATH_STR);

        let source_path = target_dir.join(format!("{}.elf", example_name));
        let dest_path = target_dir.join(format!("{}.bin", example_name));

        cmd.arg("-O");
        cmd.arg("binary");
        cmd.arg(&source_path);
        cmd.arg(&dest_path);

        info!("make_binary: {:?}", cmd);

        let status = cmd.status()?;
        if !status.success() {
            bail!("objcopy failed with error {:?}", status);
        }

        let source_dir_path = source_dir.join("pdex.bin");

        fs::copy(&dest_path, &source_dir_path)?;

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
        if let Some(target) = target {
            for asset in &target.assets {
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

    fn run_pdc(&self, source_dir: &PathBuf, dest_dir: &PathBuf) -> Result<(), Error> {
        info!("run_pdc");
        let pdc_path = playdate_sdk_path()?.join("bin").join(PDC_NAME);
        let mut cmd = Command::new(pdc_path);
        cmd.arg(source_dir);
        cmd.arg(dest_dir);

        debug!("{:?}", cmd);

        let status = cmd.status()?;
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
        let modem_path = Path::new("/dev/cu.usbmodemPDU1_Y0005491");
        let data_path = Path::new("/Volumes/PLAYDATE");

        let duration = time::Duration::from_millis(100);
        if modem_path.exists() {
            let mut cmd = Command::new(&pdutil_path);
            cmd.arg(modem_path).arg("datadisk").arg(pdx_dir);
            info!("datadisk cmd: {:#?}", cmd);
            let _ = cmd.status()?;

            while modem_path.exists() {
                thread::sleep(duration);
            }
        }

        while !data_path.exists() {
            thread::sleep(duration);
        }

        thread::sleep(duration * 5);

        let games_dir = data_path.join("Games");
        let game_device_dir = format!("{}.pdx", example_title);
        let games_target_dir = games_dir.join(&game_device_dir);
        fs::create_dir(&games_target_dir).ok();
        Self::copy_directory(&pdx_dir, &games_target_dir)?;

        let mut cmd = Command::new("diskutil");
        cmd.arg("eject").arg(&data_path);
        info!("eject cmd: {:#?}", cmd);
        let _ = cmd.status()?;

        while !modem_path.exists() {
            thread::sleep(duration);
        }

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

        #[cfg(unix)]
        let status = {
            let mut cmd = Command::new("open");
            cmd.arg("-a");
            cmd.arg("Playdate Simulator");
            cmd.arg(&pdx_path);
            cmd.status()?
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
            vec!["+nightly", "build", "-Z", "build-std"]
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

        if self.device {
            args.push("--target");
            args.push("thumbv7em-none-eabihf");
        }

        let mut command = Command::new("cargo");
        command.args(args);
        info!("build command: {:?}", command);

        let status = command.status()?;
        if !status.success() {
            bail!("cargo failed with error {:?}", status);
        }

        let overall_target_dir = project_path.join("target");
        let game_title = to_title_case(&target_name);
        let source_path = self.make_source_dir(&overall_target_dir, &game_title)?;
        let dest_path = overall_target_dir.join(format!("{}.pdx", &game_title));
        if dest_path.exists() {
            fs::remove_dir_all(&dest_path).unwrap_or_else(|_err| ());
        }
        let mut target_dir = project_path.join("target");
        let dir_name = if self.release { "release" } else { "debug" };
        if self.device {
            target_dir = target_dir.join("thumbv7em-none-eabihf").join(dir_name);
            let lib_file = target_dir.join(format!("{}lib{}.a", target_path, target_name));
            self.compile_setup(&target_dir)?;
            self.link_binary(&target_dir, &target_name, &lib_file)?;
            self.make_binary(&target_dir, &target_name, &source_path)?;
            self.copy_assets(&target_name, &project_path, &crank_manifest, &source_path)?;
            self.run_pdc(&source_path, &dest_path)?;
            if self.run {
                self.run_target(&dest_path, &game_title)?;
            }
        } else {
            target_dir = target_dir.join(dir_name).join(target_path);
            self.link_dylib(&target_dir, &target_name, &source_path)?;
            self.copy_assets(&target_name, &project_path, &crank_manifest, &source_path)?;
            self.run_pdc(&source_path, &dest_path)?;
            if self.run {
                self.run_simulator(&dest_path)?;
            }
        }

        Ok((dest_path, game_title))
    }
}

#[derive(Debug, StructOpt)]
struct Package {
    /// Build a specific example from the examples/ dir.
    #[structopt(long)]
    example: Option<String>,

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
            release: true,
            run: false,
        };
        device_build.execute(opt, crank_manifest)?;

        let sim_build = Build {
            device: false,
            example: self.example.clone(),
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
        Ok(())
    }
}

#[derive(StructOpt, Debug)]
#[structopt(name = "clank")]
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
