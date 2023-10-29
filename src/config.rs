use std::{fs, path::PathBuf};

use anyhow::{anyhow, Result};
use nix::unistd;
use serde::{Deserialize, Serialize};
use users::{os::unix::UserExt, User};

use crate::{entry::Privilege, Cli};

#[derive(Deserialize, Debug)]
pub struct Config {
    pub repo: String,
    // TODO: make this optional
    pub default_module: String,
    pub profiles: Vec<ProfileDesc>,
    pub modules: Vec<ModuleDesc>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProfileDesc {
    pub name: String,
    pub modules: Vec<String>,
}
#[derive(Deserialize, Debug)]
pub struct ModuleDesc {
    pub name: String,
    pub path: Option<String>,
}

#[derive(Debug)]
pub struct Ctx {
    pub non_root_user: User,
    pub root_user: Option<User>,

    pub _home_dir: PathBuf,
    pub canon_home_dir: PathBuf,

    pub conf: Config,
    pub _config_dir: PathBuf,
    pub dump_dir: PathBuf,
    pub profile_file: PathBuf,

    pub repo: PathBuf,
    pub canon_repo: PathBuf,
}

impl Ctx {
    pub fn new(cli: &Cli, root_user: Option<User>, non_root_user: User) -> Result<Self> {
        let home_dir = non_root_user.home_dir();
        let config_dir = {
            let config_dir = home_dir.join(".config/configma");

            if cli.config_dir.is_none() && !config_dir.exists() {
                fs::create_dir(&config_dir)?;
            }

            cli.config_dir
                .as_ref()
                .map(|p| shellexpand::tilde_with_context(p, || Some(home_dir.to_string_lossy())))
                .map(|s| s.to_string())
                .map(PathBuf::from)
                .map(|p| p.canonicalize())
                .unwrap_or(Ok(config_dir))?
        };

        let conf: Config = {
            let config_file_path = config_dir.join("config.toml");
            if config_file_path.exists() {
                let contents = std::fs::read_to_string(config_file_path)?;
                toml::from_str(&contents)?
            } else {
                return Err(anyhow!(
                    "Create a git repo and add the path to it in ~/.config/configma/config.toml."
                ));
            }
        };

        let repo = {
            let r =
                shellexpand::tilde_with_context(&conf.repo, || Some(home_dir.to_string_lossy()))
                    .into_owned();
            PathBuf::from(r)
        };

        let dump_dir = config_dir.join("dumps").join(format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
        ));

        let home_dir = non_root_user.home_dir().to_path_buf();

        let profile_file = config_dir.join("profile.active.toml");

        let s = Self {
            canon_home_dir: home_dir.canonicalize()?,
            _home_dir: home_dir,
            conf,
            _config_dir: config_dir,
            dump_dir,
            profile_file,
            canon_repo: repo.canonicalize()?,
            repo,
            root_user,
            non_root_user,
        };
        Ok(s)
    }

    pub fn escalate_privileges(&self) -> Result<Privilege<'_>> {
        let Some(root) = &self.root_user else {
            return Err(anyhow!("No root privileges"));
        };

        unistd::setegid(unistd::Gid::from_raw(root.primary_group_id()))?;
        unistd::seteuid(unistd::Uid::from_raw(root.uid()))?;

        Ok(Privilege { ctx: self })
    }
}
