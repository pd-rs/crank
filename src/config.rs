use super::Error;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

pub const CFG_DIR: &'static str = ".Playdate";
pub const CFG_FILENAME: &'static str = "config";
pub const CFG_KEY_SDK_ROOT: &'static str = "SDKRoot";

pub struct SdkCfg(HashMap<String, String>);

impl FromStr for SdkCfg {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(
            s.trim()
                .lines()
                .filter_map(|line| {
                    line.split_once("\t")
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                })
                .collect(),
        ))
    }
}

impl SdkCfg {
    pub fn sdk_path(&self) -> Option<PathBuf> {
        self.0.get(CFG_KEY_SDK_ROOT).map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let path = "/path/PlaydateSDK-dir";
        let cfg: SdkCfg = format!("{k}\t{v}\n", k = CFG_KEY_SDK_ROOT, v = path)
            .parse()
            .unwrap();
        assert_eq!(cfg.sdk_path(), Some(PathBuf::from(path)));
    }
}
