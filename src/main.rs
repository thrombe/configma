use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Specify a custom config directory
    #[arg(short, long)]
    config_dir: Option<String>,

    /// Turn debugging information on
    #[arg(short, long, action = clap::ArgAction::Count)]
    debug: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Unlink {
    /// Unlink all files from the current profile
    All,

    /// Unlink a specific file from the current profile
    Entry { src: String },
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize the config file
    ConfigInit,

    // TODO: Vec<String>
    /// Add a file to the current profile
    Add { src: String },

    // TODO: allow specifying paths from both the current profile and the src locations
    // TODO: check if the dir parent is empty, and remove it
    /// Remove a file from the current profile
    Remove { src: String },

    /// Create a new profile
    NewProfile { name: String },

    /// Switch to a different profile
    SwitchProfile { name: String },

    /// Unlink files from the current profile
    Unlink {
        #[clap(subcommand)]
        arg: Unlink,
    },

    /// Check and apply the config (if edited)
    Sync,
}

#[derive(Deserialize, Debug)]
struct Config {
    repo: Option<String>,
}

// TODO:
// - keep a log of everything that happens and undo all that if there is any kind of error.
// - 'add' should support entire directories too
//   - should it just copy every file from that dir and manage each of those individually?
//     - this has the advantage that it is easier to choose and pick whatever files are required.
//       any files that are directly added to this dir are automatically ignored. but any files
//       added to the repo are synced nicely.
//   - should it keep track of these dirs and symlink the dir to the required location somehow?

fn main() -> Result<()> {
    let cli = Cli::parse();
    // dbg!(&cli);

    let config_dir = {
        let config_dir = dirs::config_dir()
            .context("Could not find config dir.")?
            .join("configma");

        if cli.config_dir.is_none() && !config_dir.exists() {
            fs::create_dir(&config_dir)?;
        }

        cli.config_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or(config_dir)
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

    let repo = conf.repo.unwrap_or("~/.configma".into());
    let repo = shellexpand::tilde(&repo).into_owned();
    let repo = PathBuf::from(repo);
    // dbg!(&repo);

    let home_dir = dirs::home_dir().ok_or(anyhow!("Home directory not found"))?;

    let profile_file = config_dir.join("profile");
    match &cli.command {
        Commands::NewProfile { name } => {
            std::fs::create_dir(repo.join(name))?;
            return Ok(());
        }
        Commands::SwitchProfile { name } => {
            if !repo.join(name).exists() {
                return Err(anyhow!("Profile with the given name does not exist."));
            }

            if profile_file.exists() {
                let old_profile = fs::read_to_string(&profile_file)?;
                unlink_all_entries(&home_dir, &repo, &old_profile)?;

                fs::remove_file(&profile_file)?;
            }
            fs::write(&profile_file, name)?;
        }
        _ => (),
    }

    let current_profile = match fs::read_to_string(&profile_file) {
        Ok(s) => s,
        Err(_) => {
            return Err(anyhow!("Set a profile with switch-profile."));
        }
    };

    match cli.command {
        Commands::ConfigInit => todo!(),
        Commands::NewProfile { .. } => unreachable!(),
        Commands::Add { src } => {
            let src = PathBuf::from(shellexpand::tilde(&src).into_owned()).canonicalize()?;

            // Validate that the source path is within the home directory
            if !src.starts_with(home_dir.canonicalize()?) {
                return Err(anyhow!(
                    "Adding files outside of HOME directory is not allowed."
                ));
            }
            let relative_src = src.strip_prefix(home_dir.canonicalize()?)?;
            let src = home_dir.join(relative_src);

            let dest = PathBuf::from(&repo)
                .join(current_profile)
                .join(relative_src);

            // Create the necessary parent directories if they don't exist
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }

            // dbg!(&dest, &src ,&home_dir);

            // Move the source file/directory to the profile directory
            println!(
                "moving file\nsrc: {}\ndst: {}\n",
                &src.to_string_lossy(),
                &dest.to_string_lossy()
            );
            let _ = fs::copy(&src, &dest);
            fs::remove_file(&src)?;

            // Create a symlink to the original location
            #[cfg(unix)]
            std::os::unix::fs::symlink(&dest, &src)?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_file(&dest, &src)?;
        }
        Commands::Remove { src } => {
            let filename = PathBuf::from(shellexpand::tilde(&src).into_owned());
            let src = filename
                .parent()
                .unwrap()
                .canonicalize()?
                .join(filename.file_name().unwrap());

            // dbg!(&src, home_dir.canonicalize());

            // Validate that the source path is within the home directory
            if !src.starts_with(home_dir.canonicalize()?) {
                return Err(anyhow!(
                    "Removing files outside of HOME directory is not allowed."
                ));
            }
            let relative_src = src.strip_prefix(home_dir.canonicalize()?)?;
            let src = home_dir.join(relative_src);

            let dest = repo.join(current_profile).join(relative_src);

            println!(
                "restoring file\nsrc: {}\ndst: {}\n",
                &src.to_string_lossy(),
                &dest.to_string_lossy()
            );
            // Remove the symlink
            fs::remove_file(&src)?;

            // Move the file/directory back to the original location
            let _ = fs::copy(&dest, &src)?;
            fs::remove_file(&dest)?;
        }
        Commands::Unlink { arg } => match arg {
            Unlink::All => {
                unlink_all_entries(&home_dir, &repo, &current_profile)?;
            }
            Unlink::Entry { src } => {
                let filename = PathBuf::from(shellexpand::tilde(&src).into_owned());
                let src = filename
                    .parent()
                    .unwrap()
                    .canonicalize()?
                    .join(filename.file_name().unwrap());

                if !src.starts_with(home_dir.canonicalize()?) {
                    return Err(anyhow!(
                        "Unlinking files outside of HOME directory is not allowed."
                    ));
                }
                let relative_src = src.strip_prefix(home_dir.canonicalize()?)?;

                let err = Err(anyhow!("This file is not managed by configma."));
                let dest = match repo.join(current_profile).join(relative_src).canonicalize() {
                    Ok(p) => p,
                    Err(_) => return err,
                };

                // dbg!(&src, &dest);
                if dest == src.canonicalize()? && src.is_symlink() {
                    println!("deleting symlink: {}\n", &src.to_string_lossy());
                    fs::remove_file(&src)?;
                } else {
                    return err;
                }
            }
        },
        Commands::Sync | Commands::SwitchProfile { .. } => {
            let current_profile_dir = Path::new(&repo).join(current_profile).canonicalize()?;
            let walker = WalkDir::new(&current_profile_dir);
            for e in walker.into_iter() {
                let e = e?;
                if e.path().is_dir() {
                    continue;
                }
                let rel_path = e.path().strip_prefix(&current_profile_dir)?;
                let src = home_dir.join(rel_path);
                if src.exists() {
                    if src.canonicalize()? == e.path() {
                        continue;
                    } else {
                        return Err(anyhow!(format!("bad Entry: {:?}.", &src)));
                    }
                } else {
                    // Create the necessary parent directories if they don't exist
                    if let Some(parent) = src.parent() {
                        fs::create_dir_all(parent)?;
                    }

                    let dest = e.path();
                    println!(
                        "creating symlink\n src: {}\ndst: {}\n",
                        &src.to_string_lossy(),
                        &dest.to_string_lossy()
                    );
                    // Create a symlink to the original location
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(dest, &src)?;
                    #[cfg(windows)]
                    std::os::windows::fs::symlink_file(dest, &src)?;
                }
            }
        }
    }

    Ok(())
}

fn unlink_all_entries(
    home_dir: impl AsRef<Path>,
    repo: impl AsRef<Path>,
    profile: impl AsRef<str>,
) -> Result<()> {
    // Remove symlinks for all files in the profile
    let home_dir = home_dir.as_ref();
    let repo = repo.as_ref();
    let profile = profile.as_ref();
    let current_profile_dir = Path::new(&repo).join(profile).canonicalize()?;
    let walker = WalkDir::new(&current_profile_dir);
    for e in walker.into_iter() {
        let e = e?;
        if e.path().is_dir() {
            continue;
        }
        let rel_path = e.path().strip_prefix(&current_profile_dir)?;
        let src = home_dir.join(rel_path);
        if !src.is_symlink() || src.canonicalize()? != e.path() {
            return Err(anyhow!(format!("bad Entry: {:?}.", &src)));
        }
        println!("deleting symlink: {}\n", &src.to_string_lossy());
        fs::remove_file(&src)?;
    }
    Ok(())
}
