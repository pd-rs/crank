use anyhow::{anyhow, bail, Context, Error};
use inflector::cases::titlecase::to_title_case;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    thread, time,
};
use structopt::StructOpt;

fn playdate_sdk_path() -> Result<PathBuf, Error> {
    let home_dir = dirs::home_dir().ok_or(anyhow!("Can't find home dir"))?;
    Ok(home_dir.join("Developer").join("PlaydateSDK"))
}

fn playdate_c_api_path() -> Result<PathBuf, Error> {
    Ok(playdate_sdk_path()?.join("C_API"))
}

#[derive(Debug, StructOpt)]
#[structopt(about = "Crank commands")]
enum CrankCommand {
    /// Build binary targeting Playdate device or Simulator
    Build(Build),
}

#[derive(Debug, StructOpt)]
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

    fn compile_setup(&self, target_dir: &PathBuf) -> Result<(), Error> {
        let gcc_compile_static_args = "-g -c -mthumb -mcpu=cortex-m7 -mfloat-abi=hard \
        -mfpu=fpv4-sp-d16 -D__FPU_USED=1 -O2 -falign-functions=16 -fomit-frame-pointer \
        -gdwarf-2 -Wall -Wno-unused -Wstrict-prototypes -Wno-unknown-pragmas -fverbose-asm \
        -ffunction-sections -fdata-sections -DTARGET_PLAYDATE=1 -DTARGET_EXTENSION=1";
        let args_iter = gcc_compile_static_args.split(" ");
        let playdate_c_api_path = playdate_c_api_path()?;
        let setup_path = Self::setup_path()?;
        let mut command = Command::new("/usr/local/bin/arm-none-eabi-gcc");
        command
            .args(args_iter)
            .arg(setup_path)
            .arg("-I")
            .arg(playdate_c_api_path)
            .arg("-o")
            .arg(target_dir.join("setup.o"));
        println!("{:?}", command);
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

        let mut cmd = Command::new("/usr/local/bin/arm-none-eabi-gcc");
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

        println!("{:?}", cmd);

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
        let mut cmd = Command::new("/usr/local/bin/arm-none-eabi-objcopy");

        let source_path = target_dir.join(format!("{}.elf", example_name));
        let dest_path = target_dir.join(format!("{}.bin", example_name));

        cmd.arg("-O");
        cmd.arg("binary");
        cmd.arg(&source_path);
        cmd.arg(&dest_path);

        println!("{:?}", cmd);

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
        let pdx_path = overall_target_dir.join(example_title);
        fs::create_dir_all(&pdx_path)?;

        Ok(pdx_path)
    }

    fn run_pdc(&self, source_dir: &PathBuf, dest_dir: &PathBuf) -> Result<(), Error> {
        let pdc_path = playdate_sdk_path()?.join("bin").join("pdc");
        let mut cmd = Command::new(pdc_path);
        cmd.arg(source_dir);
        cmd.arg(dest_dir);

        println!("{:?}", cmd);

        let status = cmd.status()?;
        if !status.success() {
            bail!("pdc failed with error {:?}", status);
        }
        Ok(())
    }

    fn run_example(&self, pdx_dir: &PathBuf, example_title: &str) -> Result<(), Error> {
        let modem_path = Path::new("/dev/cu.usbmodem00000000001A1");
        let data_path = Path::new("/Volumes/PLAYDATE");

        let duration = time::Duration::from_millis(100);
        if modem_path.exists() {
            println!("Found modem file, switching to disk");
            let mut file = OpenOptions::new().write(true).open(&modem_path)?;
            writeln!(file, "datadisk")?;
            while modem_path.exists() {
                thread::sleep(duration);
            }
        }

        while !data_path.exists() {
            println!("Waiting for disk");
            thread::sleep(duration);
        }

        println!("Found disk");
        thread::sleep(duration * 5);

        let games_dir = data_path.join("Games");
        let games_target_dir = games_dir.join(format!("{}.pdx", example_title));
        fs::create_dir_all(&games_target_dir).context("Creating game directory on device")?;
        for entry in fs::read_dir(pdx_dir).context("Reading source game directory")? {
            let entry = entry.context("bad entry")?;
            let target_path = games_target_dir.join(entry.file_name());
            println!("Copying {:#?} to {:#?}", entry.path(), target_path);
            fs::copy(entry.path(), target_path).context("copy file")?;
        }

        let _ = Command::new("diskutil")
            .arg("eject")
            .arg(&data_path)
            .status()?;
        Ok(())
    }

    fn link_dylib(
        &self,
        target_dir: &PathBuf,
        example_name: &str,
        source_dir: &PathBuf,
    ) -> Result<(), Error> {
        let lib_target_path = target_dir.join(format!("lib{}.dylib", example_name));
        let source_dir_path = source_dir.join("pdex.dylib");
        fs::copy(&lib_target_path, &source_dir_path)?;

        let pdx_bin_path = source_dir.join("pdex.bin");
        if !pdx_bin_path.exists() {
            fs::File::create(&pdx_bin_path)?;
        }

        Ok(())
    }
    fn run_simulator(
        &self,
        pdx_path: &PathBuf,
    ) -> Result<(), Error> {
        let mut cmd = Command::new("open");
        cmd.arg("-a");
        cmd.arg("Playdate Simulator");
        cmd.arg(&pdx_path);

        let status = cmd.status()?;
        if !status.success() {
            bail!("open failed with error {:?}", status);
        }

        Ok(())
    }

    pub fn execute(&self, opt: &Opt) -> Result<(), Error> {
        let current_dir = std::fs::canonicalize(std::env::current_dir()?)?;
        let canonical_manifest_path;
        let manifest_path_str;
        let mut args = vec!["build"];

        let project_path = if let Some(manifest_path) = opt.manifest_path.as_ref() {
            args.push("--manifest-path");
            canonical_manifest_path = manifest_path.canonicalize().context(format!(
                "Cannot find manifest at path '{}'",
                manifest_path.to_string_lossy()
            ))?;
            manifest_path_str = canonical_manifest_path.to_string_lossy();
            args.push(&manifest_path_str);
            canonical_manifest_path.parent().expect("parent")
        } else {
            current_dir.as_path()
        };

        if let Some(example) = self.example.as_ref() {
            args.push("--example");
            args.push(&example)
        }

        if self.release {
            args.push("--release");
        }

        if self.device {
            args.push("--target");
            args.push("thumbv7em-none-eabihf");
        }

        println!("args = {:#?}", args);

        let status = Command::new("cargo").args(args).status()?;
        if !status.success() {
            bail!("cargo failed with error {:?}", status);
        }

        if let Some(example) = &self.example {
            let overall_target_dir = project_path.join("target");
            let example_title = to_title_case(&example);
            let source_path = self.make_source_dir(&overall_target_dir, &example_title)?;
            let dest_path = overall_target_dir.join(format!("{}.pdx", &example_title));
            let mut target_dir = project_path.join("target");
            let dir_name = if self.release { "release" } else { "debug" };
            if self.device {
                target_dir = target_dir
                    .join("thumbv7em-none-eabihf")
                    .join(dir_name)
                    .join("examples");
                let lib_file = target_dir.join(format!("lib{}.a", example));
                self.compile_setup(&target_dir)?;
                self.link_binary(&target_dir, example, &lib_file)?;
                self.make_binary(&target_dir, example, &source_path)?;
                self.run_pdc(&source_path, &dest_path)?;
                self.run_example(&dest_path, &example_title)?;
            } else {
                target_dir = target_dir.join(dir_name).join("examples");
                self.link_dylib(&target_dir, example, &source_path)?;
                self.run_pdc(&source_path, &dest_path)?;
                self.run_simulator(&dest_path)?;
            }
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
    println!("opt = {:#?}", opt);

    match &opt.cmd {
        CrankCommand::Build(build) => {
            build.execute(&opt)?;
        }
    }

    Ok(())
}
