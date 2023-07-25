use std::{
    collections::HashSet,
    fs,
    ops::Deref,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;

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
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Add paths to the current profile
    Add {
        #[clap(required = true)]
        src: Vec<String>,
    },

    // TODO: allow specifying paths from both the current profile and the src locations
    // TODO: check if the parent dir is empty, and remove it
    /// Remove paths from the current profile
    Remove {
        #[clap(required = true)]
        src: Vec<String>,
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

    /// Unlink files from the current profile
    #[clap(group = clap::ArgGroup::new("target").required(true))]
    Unlink {
        /// Unlink all files from the current profile
        #[clap(group = "target")]
        #[arg(long, short, default_value_t = false)]
        all: bool,

        /// Unlink a specific file from the current profile
        #[clap(group = "target")]
        src: Option<String>,
    },

    /// Check and apply the config (if edited)
    Sync {
        /// overwrite files
        #[arg(long, short, default_value_t = false)]
        force: bool,
    },
}

#[derive(Deserialize, Debug)]
struct Config {
    repo: String,
}

const DIR_STUB: &str = "configma_dir.stub";

fn generate_entry_set(profile: impl AsRef<Path>) -> Result<HashSet<PathBuf>> {
    let mut set = HashSet::new();

    let mut dir_buff = Vec::new();
    let mut dir_buff_iter = vec![profile.as_ref().to_path_buf()];

    while !dir_buff_iter.is_empty() {
        for dir in dir_buff_iter.iter() {
            for e in fs::read_dir(dir)? {
                let e = e?;
                let ft = e.file_type()?;
                let p = e.path();
                let rel_path = p.strip_prefix(&profile)?.to_path_buf();

                if ft.is_file() {
                    set.insert(rel_path);
                } else if ft.is_dir() {
                    if p.join(DIR_STUB).exists() {
                        set.insert(rel_path);
                    } else {
                        dir_buff.push(p);
                    }
                } else if ft.is_symlink() {
                    println!(
                        "Warning: ignoring symlink: {}\n",
                        e.path().to_string_lossy()
                    );
                } else {
                    println!("ignoring path: {}\n", e.path().to_string_lossy());
                }
            }
        }

        dir_buff_iter.clear();
        std::mem::swap(&mut dir_buff, &mut dir_buff_iter);
    }

    Ok(set)
}

#[derive(Debug)]
struct Ctx {
    _home_dir: PathBuf,
    canon_home_dir: PathBuf,

    _conf: Config,
    _config_dir: PathBuf,
    dump_dir: PathBuf,
    profile_file: PathBuf,

    repo: PathBuf,
    canon_repo: PathBuf,
}

impl Ctx {
    fn new(cli: &Cli) -> Result<Self> {
        let config_dir = {
            let config_dir = dirs::config_dir()
                .context("Could not find config dir.")?
                .join("configma");

            if cli.config_dir.is_none() && !config_dir.exists() {
                fs::create_dir(&config_dir)?;
            }

            cli.config_dir
                .as_ref()
                .map(shellexpand::tilde)
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
            let r = shellexpand::tilde(&conf.repo).into_owned();
            PathBuf::from(r)
        };

        let dump_dir = config_dir.join("dumps").join(format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
        ));

        let home_dir = dirs::home_dir().ok_or(anyhow!("Home directory not found"))?;

        let profile_file = config_dir.join("profile");

        let s = Self {
            canon_home_dir: home_dir.canonicalize()?,
            _home_dir: home_dir,
            _conf: conf,
            _config_dir: config_dir,
            dump_dir,
            profile_file,
            canon_repo: repo.canonicalize()?,
            repo,
        };
        Ok(s)
    }

    fn resolver(self) -> Result<Resolver> {
        let profile_dir = {
            let current_profile = fs::read_to_string(&self.profile_file)?;
            self.canon_repo.join(current_profile)
        };

        let entries = generate_entry_set(&profile_dir)?;

        Ok(Resolver {
            ctx: self,
            profile_dir,
            entries,
        })
    }
}

#[derive(Debug)]
struct Entry {
    src: PathBuf,
    relative: PathBuf,
    dest: PathBuf,
}

#[derive(Debug)]
struct Resolver {
    ctx: Ctx,

    profile_dir: PathBuf,
    entries: HashSet<PathBuf>,
}

impl Deref for Resolver {
    type Target = Ctx;

    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

enum PathResolutionError {
    InRepo,
    OutsideHome,
    OutsideRepo,
}

impl Resolver {
    fn resolve_path(&self, path: impl AsRef<str>) -> Result<PathBuf> {
        let filename = PathBuf::from(shellexpand::tilde(path.as_ref()).into_owned());
        let src = filename
            .parent()
            .unwrap()
            .canonicalize()?
            .join(filename.file_name().unwrap());
        Ok(src)
    }

    fn entry_from_dest(&self, dest: impl AsRef<Path>) -> Result<Entry, PathResolutionError> {
        let dest = dest.as_ref();

        if !dest.starts_with(&self.canon_repo) {
            return Err(PathResolutionError::OutsideRepo);
        }

        let relative = dest.strip_prefix(&self.profile_dir).unwrap();

        Ok(Entry {
            src: self.canon_home_dir.join(relative),
            relative: relative.to_path_buf(),
            dest: dest.to_path_buf(),
        })
    }

    fn entry_from_src(&self, src: impl AsRef<Path>) -> Result<Entry, PathResolutionError> {
        let src = src.as_ref();

        if src.starts_with(&self.canon_repo) {
            return Err(PathResolutionError::InRepo);
        }

        // Validate that the source path is within the home directory
        if !src.starts_with(&self.canon_home_dir) {
            return Err(PathResolutionError::OutsideHome);
        }

        let relative_src = src.strip_prefix(&self.canon_home_dir).unwrap();
        let dest = self.profile_dir.join(relative_src);

        Ok(Entry {
            src: src.to_path_buf(),
            relative: relative_src.to_path_buf(),
            dest,
        })
    }

    fn entry_from_relative(&self, rel: impl AsRef<Path>) -> Entry {
        Entry {
            src: self.canon_home_dir.join(rel.as_ref()),
            relative: rel.as_ref().to_path_buf(),
            dest: self.profile_dir.join(rel.as_ref()),
        }
    }

    fn entry(&self, path_str: impl AsRef<str>) -> Result<Entry> {
        let path = self.resolve_path(&path_str)?;
        match self.entry_from_dest(&path) {
            Ok(p) => Ok(p),
            Err(PathResolutionError::OutsideRepo) => match self.entry_from_src(&path) {
                Ok(p) => Ok(p),
                Err(PathResolutionError::OutsideHome) => Err(anyhow!(
                    "failed to find entry for path: {}",
                    path_str.as_ref()
                )),
                Err(_) => unreachable!(),
            },
            Err(_) => unreachable!(),
        }
    }

    fn contains(&self, e: &Entry) -> bool {
        self.entries.contains(&e.relative)
    }

    /// creates new symlinks for any entry that does not have a symlink
    fn sync(&self, force: bool) -> Result<()> {
        for p in self.entries.iter() {
            let e = self.entry_from_relative(p);

            fs::create_dir_all(e.src.parent().unwrap())?;

            if !e.src.exists() {
                println!(
                    "creating symlink\n  src: {:?}\n  dst: {:?}",
                    &e.src, &e.dest
                );
                std::os::unix::fs::symlink(&e.dest, &e.src)?;
                continue;
            }

            if e.src.canonicalize()? == e.dest {
                continue;
            }

            println!(
                "creating symlink\n  src: {:?}\n  dst: {:?}",
                &e.src, &e.dest
            );

            if !force {
                return Err(anyhow!("bad Entry: {:?}.", &e.src));
            }

            let dump_to = self.dump_dir.join(&e.relative);

            println!(
                "moving contents to dump\n  src: {:?}\n  dump: {:?}",
                &e.src, &dump_to
            );

            fs::create_dir_all(dump_to.parent().unwrap())?;

            if e.src.is_file() || e.src.is_symlink() {
                let _ = fs::copy(&e.src, &dump_to)?;
                fs::remove_file(&e.src)?;
            } else if e.src.is_dir() {
                fs_extra::dir::copy(
                    &e.src,
                    &dump_to,
                    &fs_extra::dir::CopyOptions::new().copy_inside(true),
                )?;
                fs::remove_dir_all(&e.src)?;
            } else {
                return Err(anyhow!(
                    "cannot handle this type of file or whatever: {:?}",
                    &e.src
                ));
            }
            println!();

            std::os::unix::fs::symlink(&e.dest, &e.src)?;
        }

        Ok(())
    }

    fn unlink_all(&self, ignore_non_links: bool) -> Result<()> {
        for p in self.entries.iter() {
            let e = self.entry_from_relative(p);
            if !e.src.is_symlink() || e.src.canonicalize()? != e.dest {
                if ignore_non_links {
                    continue;
                } else {
                    return Err(anyhow!("bad Entry: {:?}.", p));
                }
            }
            println!("deleting symlink: {:?}\n", &e.src);
            fs::remove_file(&e.src)?;
        }

        Ok(())
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let ctx = Ctx::new(&cli)?;

    if !ctx.profile_file.exists() {
        match &cli.command {
            Command::NewProfile { name } => {
                std::fs::create_dir(ctx.repo.join(name))?;
                fs::write(&ctx.profile_file, name)?;
            }
            Command::SwitchProfile { name, force } => {
                if !ctx.repo.join(name).exists() {
                    return Err(anyhow!("Profile with the given name does not exist."));
                }
                fs::write(&ctx.profile_file, name)?;

                ctx.resolver()?.sync(*force)?;
            }
            _ => {
                return Err(anyhow!(
                    "Set a profile with switch-profile, or new-profile."
                ))
            }
        }

        return Ok(());
    }

    let rsv = ctx.resolver()?;

    match cli.command {
        Command::NewProfile { .. } => (),
        Command::SwitchProfile { name, force } => {
            if !rsv.repo.join(&name).exists() {
                return Err(anyhow!("Profile with the given name does not exist."));
            }

            rsv.unlink_all(force)?;

            fs::remove_file(&rsv.profile_file)?;
            fs::write(&rsv.profile_file, &name)?;

            let rsv = rsv.ctx.resolver()?;

            rsv.sync(force)?;
        }
        Command::Sync { force } => {
            rsv.sync(force)?;
        }
        Command::Unlink { all, src } => {
            if let Some(src) = src {
                let e = rsv.entry(&src)?;
                if rsv.contains(&e) {
                    println!("deleting symlink: {}\n", &src);
                    fs::remove_file(&e.src)?;
                } else {
                    return Err(anyhow!("file {} not maintained by configma", &src));
                }
            } else if all {
                rsv.unlink_all(false)?;
            } else {
                unreachable!();
            }
        }
        Command::Remove { src } => {
            for src in src.iter() {
                let e = rsv.entry(src)?;
                if rsv.contains(&e) {
                    println!("restoring path\n  src: {:?}\n  dst: {:?}\n", e.src, e.dest,);

                    fs::remove_file(&e.src)?;
                    if e.dest.is_dir() {
                        fs::remove_file(e.dest.join(DIR_STUB))?;
                        fs_extra::dir::copy(
                            &e.dest,
                            &e.src,
                            &fs_extra::dir::CopyOptions::new().copy_inside(true),
                        )?;
                        fs::remove_dir_all(&e.dest)?;
                    } else if e.dest.is_file() {
                        fs::copy(&e.dest, &e.src)?;
                        fs::remove_file(&e.dest)?;
                    } else {
                        return Err(anyhow!(
                            "cannot handle this type of file or whatever: {}",
                            &src
                        ));
                    }
                } else {
                    return Err(anyhow!("file {} not maintained by configma", &src));
                }
            }
        }
        Command::Add { src } => {
            for src in src.iter() {
                let e = match rsv.entry_from_src(rsv.resolve_path(src)?) {
                    Ok(e) => e,
                    Err(PathResolutionError::InRepo) => {
                        println!("the path {} is already in the repo.", src);
                        continue;
                    }
                    Err(PathResolutionError::OutsideHome) => {
                        return Err(anyhow!(
                            "Adding paths outside of HOME directory is not allowed."
                        ))
                    }
                    Err(PathResolutionError::OutsideRepo) => unreachable!(),
                };

                let mut p = rsv.profile_dir.clone();
                for c in e.relative.parent().unwrap().components() {
                    let std::path::Component::Normal(c) = c else {unreachable!()};
                    p.push(c);

                    p.push(DIR_STUB);
                    if p.exists() {
                        return Err(anyhow!(
                            "path is already in a directory managed by configma\n  src: {}\n  dir: {:?}\n",
                            src,
                            p,
                        ));
                    }
                    p.pop();
                }

                if rsv.contains(&e) {
                    println!("path is already maintained by configma: {}\n", src);
                    continue;
                }

                println!("moving path\n  src: {:?}\n  dst: {:?}\n", &e.src, &e.dest);

                fs::create_dir_all(e.dest.parent().unwrap())?;

                if e.src.is_file() {
                    fs::copy(&e.src, &e.dest)?;
                    fs::remove_file(&e.src)?;
                } else if e.src.is_dir() {
                    fs_extra::dir::copy(
                        &e.src,
                        &e.dest,
                        &fs_extra::dir::CopyOptions::new().copy_inside(true),
                    )?;
                    fs::remove_dir_all(&e.src)?;
                    let _ = fs::File::create(e.dest.join(DIR_STUB))?;
                } else {
                    return Err(anyhow!(
                        "cannot handle this type of file or whatever: {}",
                        &src
                    ));
                }

                std::os::unix::fs::symlink(&e.dest, &e.src)?;
            }
        }
    }

    Ok(())
}
