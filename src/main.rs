use std::{
    collections::HashSet,
    fs,
    ops::Deref,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use nix::unistd;
use serde::Deserialize;
use users::{os::unix::UserExt, User};

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
const HOME: &str = "home";

fn generate_entry_set(parent_dir: impl AsRef<Path>) -> Result<HashSet<PathBuf>> {
    let mut set = HashSet::new();

    let mut dir_buff = Vec::new();
    let mut dir_buff_iter = vec![parent_dir.as_ref().to_path_buf()];

    while !dir_buff_iter.is_empty() {
        for dir in dir_buff_iter.iter() {
            for e in fs::read_dir(dir)? {
                let e = e?;
                let ft = e.file_type()?;
                let p = e.path();
                let rel_path = p.strip_prefix(&parent_dir)?.to_path_buf();

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
    non_root_user: User,
    root_user: Option<User>,

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
    fn new(cli: &Cli, root_user: Option<User>, non_root_user: User) -> Result<Self> {
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
            root_user,
            non_root_user,
        };
        Ok(s)
    }

    fn resolver(self) -> Result<Resolver> {
        let profile_dir = {
            let current_profile = fs::read_to_string(&self.profile_file)?;
            self.canon_repo.join(current_profile)
        };

        let profile_home = profile_dir.join(HOME);
        if !profile_home.exists() {
            fs::create_dir(&profile_home)?;
        }
        let home_entries = generate_entry_set(profile_home)?;

        let mut entries = HashSet::new();
        for dir in fs::read_dir(&profile_dir)? {
            let dir = dir?;
            let path = dir.path();

            // None only if path ends in '..'
            if path.file_name().unwrap() == HOME {
                continue;
            }

            if path.is_file() {
                entries.insert(path.strip_prefix(&profile_dir)?.to_path_buf());
            } else if path.is_dir() {
                let dir_entries = generate_entry_set(&path)?;
                entries.extend(
                    dir_entries
                        .into_iter()
                        .map(|p| PathBuf::from(path.file_name().unwrap()).join(p)),
                );
            } else {
                println!("ignoring unhandlable path: {:?}", &path);
            }
        }

        Ok(Resolver {
            ctx: self,
            profile_dir,
            home_entries,
            non_home_entries: entries,
        })
    }

    fn escalate_privileges(&self) -> Result<Privilege<'_>> {
        let Some(root) = &self.root_user else {
            return Err(anyhow!("No root privileges"));
        };

        unistd::setegid(unistd::Gid::from_raw(root.primary_group_id()))?;
        unistd::seteuid(unistd::Uid::from_raw(root.uid()))?;

        Ok(Privilege { ctx: self })
    }
}

#[derive(Debug)]
struct Privilege<'a> {
    ctx: &'a Ctx,
}
impl<'a> Drop for Privilege<'a> {
    fn drop(&mut self) {
        unistd::setegid(unistd::Gid::from_raw(
            self.ctx.non_root_user.primary_group_id(),
        ))
        .expect("could not drop privileges");

        unistd::seteuid(unistd::Uid::from_raw(self.ctx.non_root_user.uid()))
            .expect("could not drop privileges");
    }
}

#[derive(Debug, Clone)]
enum RelativePath {
    Home(PathBuf),
    NonHome(PathBuf),
}

impl RelativePath {
    fn relative(self) -> PathBuf {
        match self {
            RelativePath::Home(p) => PathBuf::from(HOME).join(p),
            RelativePath::NonHome(p) => p,
        }
    }
}

#[derive(Debug)]
struct Entry {
    src: PathBuf,
    relative: RelativePath,
    dest: PathBuf,
}

impl Entry {
    fn get_priv<'a>(&self, ctx: &'a Ctx) -> Result<Option<Privilege<'a>>> {
        match &self.relative {
            RelativePath::Home(_) => Ok(None),
            RelativePath::NonHome(_) => {
                // let pri = ctx.escalate_privileges();
                // if the parent of the file/dir is root - then escilate privileges
                if self
                    .src
                    .ancestors()
                    .skip(1)
                    .find(|p| p.exists())
                    .map(nix::sys::stat::stat)
                    .transpose()?
                    .map(|s| s.st_uid)
                    .map(unistd::Uid::from_raw)
                    .map(unistd::Uid::is_root)
                    .context("some ancestor of the path must exist")?
                {
                    // Ok(Some(pri?))
                    Ok(Some(ctx.escalate_privileges()?))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn rm_src_file(&self, ctx: &Ctx) -> Result<()> {
        let p = self.get_priv(ctx)?;

        fs::remove_file(&self.src)?;

        drop(p);
        Ok(())
    }

    fn rm_src_dir_all(&self, ctx: &Ctx) -> Result<()> {
        let p = self.get_priv(ctx)?;

        fs::remove_dir_all(&self.src)?;

        drop(p);
        Ok(())
    }

    fn copy_file_to_src(&self, ctx: &Ctx) -> Result<()> {
        let p = self.get_priv(ctx)?;

        fs::copy(&self.dest, &self.src)?;

        drop(p);
        Ok(())
    }

    fn copy_dir_to_src(&self, ctx: &Ctx) -> Result<()> {
        let p = self.get_priv(ctx)?;

        fs_extra::dir::copy(
            &self.dest,
            &self.src,
            &fs_extra::dir::CopyOptions::new()
                .copy_inside(false)
                .content_only(true),
        )?;

        drop(p);
        Ok(())
    }

    fn symlink_to_src(&self, ctx: &Ctx) -> Result<()> {
        let p = self.get_priv(ctx)?;

        std::os::unix::fs::symlink(&self.dest, &self.src)?;

        drop(p);
        Ok(())
    }
}

#[derive(Debug)]
struct Resolver {
    ctx: Ctx,

    profile_dir: PathBuf,
    home_entries: HashSet<PathBuf>,
    non_home_entries: HashSet<PathBuf>,
}

impl Deref for Resolver {
    type Target = Ctx;

    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

enum PathResolutionError {
    InRepo,
    OutsideRepo,
}

impl Resolver {
    fn resolve_path(&self, path: impl AsRef<str>) -> Result<PathBuf> {
        let filename = PathBuf::from(
            shellexpand::tilde_with_context(path.as_ref(), || {
                Some(self.canon_home_dir.to_string_lossy())
            })
            .into_owned(),
        );
        let src = filename
            .parent()
            .map(|p| {
                if p.to_string_lossy().is_empty() {
                    PathBuf::from("./").canonicalize()
                } else {
                    p.canonicalize()
                }
            })
            .unwrap()?
            .join(filename.file_name().unwrap());
        Ok(src)
    }

    fn entry_from_dest(&self, dest: impl AsRef<Path>) -> Result<Entry, PathResolutionError> {
        let dest = dest.as_ref();

        if !dest.starts_with(&self.canon_repo) {
            return Err(PathResolutionError::OutsideRepo);
        }

        let relative = dest.strip_prefix(&self.profile_dir).unwrap();
        let (src, relative) = match relative.starts_with(HOME) {
            true => {
                let stripped = relative.strip_prefix(HOME).unwrap().to_path_buf();
                (
                    self.canon_home_dir.join(&stripped),
                    RelativePath::Home(stripped),
                )
            }
            false => (
                PathBuf::from("/").join(relative),
                RelativePath::NonHome(relative.to_path_buf()),
            ),
        };

        Ok(Entry {
            src,
            relative,
            dest: dest.to_path_buf(),
        })
    }

    fn entry_from_src(&self, src: impl AsRef<Path>) -> Result<Entry, PathResolutionError> {
        let src = src.as_ref();

        if src.starts_with(&self.canon_repo) {
            return Err(PathResolutionError::InRepo);
        }

        let (dest, relative) = match src.starts_with(&self.canon_home_dir) {
            true => {
                let stripped = src.strip_prefix(&self.canon_home_dir).unwrap();
                (
                    self.profile_dir.join(HOME).join(stripped),
                    RelativePath::Home(stripped.to_path_buf()),
                )
            }
            false => {
                let stripped = src.strip_prefix("/").expect("path must be absolute");
                (
                    self.profile_dir.join(stripped),
                    RelativePath::NonHome(stripped.to_path_buf()),
                )
            }
        };

        Ok(Entry {
            src: src.to_path_buf(),
            relative,
            dest,
        })
    }

    fn entry_from_relative(&self, rel: &RelativePath) -> Entry {
        match rel {
            RelativePath::Home(p) => Entry {
                src: self.canon_home_dir.join(p),
                relative: rel.clone(),
                dest: self.profile_dir.join(HOME).join(p),
            },
            RelativePath::NonHome(p) => Entry {
                src: PathBuf::from("/").join(p),
                relative: rel.clone(),
                dest: self.profile_dir.join(p),
            },
        }
    }

    fn entry(&self, path_str: impl AsRef<str>) -> Result<Entry> {
        let path = self.resolve_path(&path_str)?;
        match self.entry_from_dest(&path) {
            Ok(p) => Ok(p),
            Err(PathResolutionError::OutsideRepo) => match self.entry_from_src(&path) {
                Ok(p) => Ok(p),
                Err(_) => unreachable!(),
            },
            Err(_) => unreachable!(),
        }
    }

    fn contains(&self, e: &Entry) -> bool {
        match &e.relative {
            RelativePath::Home(p) => self.home_entries.contains(p),
            RelativePath::NonHome(p) => self.non_home_entries.contains(p),
        }
    }

    /// creates new symlinks for any entry that does not have a symlink
    fn sync(&self, force: bool) -> Result<()> {
        for e in self
            .home_entries
            .iter()
            .map(|p| self.entry_from_relative(&RelativePath::Home(p.to_path_buf())))
            .chain(
                self.non_home_entries
                    .iter()
                    .map(|p| self.entry_from_relative(&RelativePath::NonHome(p.to_path_buf()))),
            )
        {
            let privilege = e.get_priv(&self.ctx)?;
            fs::create_dir_all(e.src.parent().unwrap())?;
            drop(privilege);

            if !e.src.exists() {
                println!(
                    "creating symlink\n  src: {:?}\n  dst: {:?}",
                    &e.src, &e.dest
                );
                e.symlink_to_src(&self.ctx)?;
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

            let dump_to = self.dump_dir.join(e.relative.clone().relative());

            println!(
                "moving contents to dump\n  src: {:?}\n  dump: {:?}",
                &e.src, &dump_to
            );

            fs::create_dir_all(dump_to.parent().unwrap())?;

            if e.src.is_file() || e.src.is_symlink() {
                let _ = fs::copy(&e.src, &dump_to)?;
                e.rm_src_file(&self.ctx)?;
            } else if e.src.is_dir() {
                fs_extra::dir::copy(
                    &e.src,
                    &dump_to,
                    &fs_extra::dir::CopyOptions::new()
                        .copy_inside(false)
                        .content_only(true),
                )?;
                e.rm_src_dir_all(&self.ctx)?;
            } else {
                return Err(anyhow!(
                    "cannot handle this type of file or whatever: {:?}",
                    &e.src
                ));
            }
            println!();

            e.symlink_to_src(&self.ctx)?;
        }

        Ok(())
    }

    fn unlink_all(&self, ignore_non_links: bool) -> Result<()> {
        for e in self
            .home_entries
            .iter()
            .map(|p| self.entry_from_relative(&RelativePath::Home(p.to_path_buf())))
            .chain(
                self.non_home_entries
                    .iter()
                    .map(|p| self.entry_from_relative(&RelativePath::NonHome(p.to_path_buf()))),
            )
        {
            if !e.src.is_symlink() || e.src.canonicalize()? != e.dest {
                if ignore_non_links {
                    continue;
                } else {
                    return Err(anyhow!("bad Entry: {:?}.", &e.relative));
                }
            }

            println!("deleting symlink: {:?}\n", &e.src);
            e.rm_src_file(&self.ctx)?;
        }

        Ok(())
    }
}

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
                    e.rm_src_file(&rsv.ctx)?;
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

                    e.rm_src_file(&rsv.ctx)?;
                    if e.dest.is_dir() {
                        fs::remove_file(e.dest.join(DIR_STUB))?;
                        e.copy_dir_to_src(&rsv.ctx)?;
                        fs::remove_dir_all(&e.dest)?;
                    } else if e.dest.is_file() {
                        e.copy_file_to_src(&rsv.ctx)?;
                        fs::remove_file(&e.dest)?;
                    } else {
                        return Err(anyhow!(
                            "cannot handle this type of file or whatever: {}",
                            &src
                        ));
                    }

                    // remove empty parent dirs
                    let mut parent = e.relative.clone().relative();
                    while parent.pop() && !parent.to_string_lossy().is_empty() {
                        let p = rsv.profile_dir.join(&parent);
                        if p.read_dir()?.count() == 0 {
                            fs::remove_dir(p)?;
                        } else {
                            break;
                        }
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
                    Err(PathResolutionError::OutsideRepo) => unreachable!(),
                };

                let mut p = rsv.profile_dir.clone();
                for c in e.relative.clone().relative().parent().unwrap().components() {
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
                    // the files should have read permissions without root
                    fs::copy(&e.src, &e.dest)?;
                    e.rm_src_file(&rsv.ctx)?;
                } else if e.src.is_dir() {
                    // the files should have read permissions without root
                    fs_extra::dir::copy(
                        &e.src,
                        &e.dest,
                        &fs_extra::dir::CopyOptions::new()
                            .copy_inside(false)
                            .content_only(true),
                    )?;
                    e.rm_src_dir_all(&rsv.ctx)?;
                    let _ = fs::File::create(e.dest.join(DIR_STUB))?;
                } else {
                    return Err(anyhow!(
                        "cannot handle this type of file or whatever: {}",
                        &src
                    ));
                }

                e.symlink_to_src(&rsv.ctx)?;
            }
        }
    }

    Ok(())
}
