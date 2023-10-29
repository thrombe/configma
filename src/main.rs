use std::fs;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use config::{Ctx, ProfileDesc};
use nix::unistd;
use profile::Profile;

mod config;
mod entry;
mod module;
mod profile;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Specify a custom config directory
    #[arg(short, long)]
    pub config_dir: Option<String>,

    // /// Turn debugging information on
    // #[arg(short, long, action = clap::ArgAction::Count)]
    // pub debug: u8,
    #[command(subcommand)]
    pub command: Command,
    // #[arg(long = "dry")]
    // pub dry_run: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Add entry to module
    Add {
        #[clap(required = true)]
        src: Vec<String>,

        #[clap(long, short)]
        module: Option<String>,
    },

    // - [Support for using ArgGroup as Enum with derive](https://github.com/clap-rs/clap/issues/2621#issuecomment-1074671496)
    /// Remove entry from module
    #[clap(group = clap::ArgGroup::new("module-method").multiple(false).required(true))]
    Remove {
        #[clap(required = true)]
        src: Vec<String>,

        // specify module to remove the entry from
        #[clap(long, short, group = "module-method")]
        module: Option<String>,

        // remove entry from the active module with highest precedence
        #[clap(long, short, group = "module-method")]
        active: bool,

        // remove entry from default module
        #[clap(long, short, group = "module-method")]
        default: bool,
        // TODO: choose module with fzf
        // #[clap(long, short, group = "module-method", default_value_t = true)]
        // choose: bool,
    },

    /// Create a new profile
    NewProfile {
        /// Name of the new profile
        name: String,
    },

    /// Switch to a different profile
    SwitchProfile {
        name: String,

        /// overwrite files
        #[arg(long, short, default_value_t = false)]
        force: bool,
    },

    /// Check and apply the config (if edited)
    Sync {
        /// overwrite files
        #[arg(long, short, default_value_t = false)]
        force: bool,
    },
}

// TODO: edit readme to remove stuff about a single file + other stuff

fn main() -> Result<()> {
    let (root_u, non_root_u) = if unistd::geteuid().is_root() {
        let non_root_user = users::get_user_by_name(&std::env::var("SUDO_USER")?)
            .context("configma must be run as a non root user or using sudo")?;
        let root_user =
            users::get_user_by_name(&std::env::var("USER")?).context("USER is not set :/")?;

        // drop effective privileges until required
        unistd::setegid(unistd::Gid::from_raw(non_root_user.primary_group_id()))?;
        unistd::seteuid(unistd::Uid::from_raw(non_root_user.uid()))?;

        (Some(root_user), non_root_user)
    } else {
        let user =
            users::get_user_by_name(&std::env::var("USER")?).context("USER is not set :/")?;
        (None, user)
    };

    let cli = Cli::parse();
    let ctx = Ctx::new(&cli, root_u, non_root_u)?;

    if !ctx.profile_file.exists() {
        match &cli.command {
            Command::NewProfile { name } => {
                std::fs::create_dir(ctx.repo.join(name))?;
                let prof = ProfileDesc {
                    name: name.to_owned(),
                    modules: Default::default(),
                };
                fs::write(&ctx.profile_file, toml::to_string_pretty(&prof)?)?;

                return Ok(());
            }
            Command::SwitchProfile { name, .. } => {
                let Some(_) = ctx.conf.profiles.iter().find(|p| p.name.as_str() == name) else {
                    return Err(anyhow!("profile with name: '{}' does not exist.", &name));
                };
                let prof = ProfileDesc {
                    name: name.to_owned(),
                    modules: Default::default(),
                };
                fs::write(&ctx.profile_file, toml::to_string_pretty(&prof)?)?;
            }
            _ => return Err(anyhow!("Set a profile with switch-profile.")),
        }
    }

    let active = fs::read_to_string(&ctx.profile_file)?;
    let active_conf = toml::from_str::<ProfileDesc>(&active)?;

    let mut profile = match &cli.command {
        Command::SwitchProfile { name, .. } => {
            let Some(required) = ctx.conf.profiles.iter().find(|p| p.name.as_str() == name) else {
                return Err(anyhow!(
                    "profile with name: '{}' not found in configs.",
                    &active_conf.name
                ));
            };

            Profile::new(active_conf, required.clone(), &ctx)?
        }
        Command::Add { .. }
        | Command::Remove { .. }
        | Command::NewProfile { .. }
        | Command::Sync { .. } => {
            let Some(required) = ctx
                .conf
                .profiles
                .iter()
                .find(|p| p.name == active_conf.name)
            else {
                return Err(anyhow!(
                    "profile with name: '{}' not found in configs.",
                    &active_conf.name
                ));
            };

            Profile::new(active_conf, required.clone(), &ctx)?
        }
    };

    if ctx
        .conf
        .default_module
        .as_ref()
        .map(|m| !profile.required_conf.modules.contains(m))
        .unwrap_or(false)
    {
        return Err(anyhow!("profile must contain the default module."));
    }

    match cli.command {
        Command::NewProfile { .. } => (),
        Command::SwitchProfile { force, .. } => {
            profile.validate()?;
            profile.sync(force, &ctx)?;
        }
        Command::Sync { force } => {
            profile.validate()?;
            profile.sync(force, &ctx)?;
        }
        Command::Remove {
            src,
            module,
            active,
            default: def,
        } => {
            for src in src.iter() {
                if let Some(name) = module.as_ref() {
                    profile.remove(src, &ctx, name)?;
                } else if def {
                    let name = ctx.conf.default_module.as_ref().context(
                        "no module specified. set default_module in configs or use -m flag",
                    )?;
                    profile.remove(src, &ctx, name)?;
                } else if active {
                    profile.remove_from_active(src, &ctx)?;
                } else {
                    unreachable!()
                };
            }
        }
        Command::Add { src, module: name } => {
            let name = name
                .as_ref()
                .or(ctx.conf.default_module.as_ref())
                .context("no module specified. set default_module in configs or use -m flag")?;
            for src in src.iter() {
                profile.add(src, &ctx, name)?;
            }
        }
    }

    Ok(())
}
