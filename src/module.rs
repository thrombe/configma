use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::Ctx,
    entry::{generate_entry_set, Convenience, Entry, RelativePath, HOME, STUB},
};

#[derive(Deserialize, Serialize, Debug)]
pub struct Module {
    pub name: String,
    pub module_dir: PathBuf,
    pub home_entries: HashSet<PathBuf>,
    pub non_home_entries: HashSet<PathBuf>,
}

pub enum PathResolutionError {
    InRepo,
    OutsideRepo,
}

impl Module {
    pub fn new(name: String, repo: impl AsRef<Path>) -> Result<Self> {
        let repo = repo.as_ref();
        if !repo.exists() {
            return Err(anyhow!("path does not exist: {:?}", repo));
        }
        let module_dir = repo.join(&name);

        let home = module_dir.join(HOME);
        if !home.exists() {
            fs::create_dir(&home)?;
        }

        let home_entries = generate_entry_set(home)?;

        let mut entries = HashSet::new();
        for dir in fs::read_dir(&module_dir)? {
            let dir = dir?;
            let path = dir.path();

            if path.name() == HOME {
                continue;
            }

            if path.is_file() {
                entries.insert(path.strip_prefix(&module_dir)?.to_path_buf());
            } else if path.is_dir() {
                let dir_entries = generate_entry_set(&path)?;
                entries.extend(
                    dir_entries
                        .into_iter()
                        .map(|p| PathBuf::from(path.file_name().expect("no file name")).join(p)),
                );
            } else {
                println!("ignoring unhandlable path: {:?}", &path);
            }
        }

        let s = Self {
            name,
            module_dir,
            home_entries,
            non_home_entries: entries,
        };
        Ok(s)
    }

    pub fn contains(&self, e: &Entry) -> bool {
        match &e.relative {
            RelativePath::Home(p) => self.home_entries.contains(p),
            RelativePath::NonHome(p) => self.non_home_entries.contains(p),
        }
    }

    /// creates new symlinks for any entry that does not have a symlink
    pub fn sync(&self, force: bool, ctx: &Ctx) -> Result<()> {
        for e in
            self.home_entries
                .iter()
                .map(|p| self.entry_from_relative(&RelativePath::Home(p.to_path_buf()), ctx))
                .chain(self.non_home_entries.iter().map(|p| {
                    self.entry_from_relative(&RelativePath::NonHome(p.to_path_buf()), ctx)
                }))
        {
            let privilege = e.get_priv(ctx)?;
            fs::create_dir_all(e.src.parent().unwrap())?;
            drop(privilege);

            if !e.src.exists() {
                println!(
                    "creating symlink\n  src: {:?}\n  dst: {:?}",
                    &e.src, &e.dest
                );
                e.symlink_to_src(ctx)?;
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
                return Err(anyhow!(
                    "there is already a file/dir at: {:?}. use -f flag to force sync",
                    &e.src
                ));
            }

            let dump_to = ctx.dump_dir.join(e.relative.clone().relative());

            println!(
                "moving contents to dump\n  src: {:?}\n  dump: {:?}",
                &e.src, &dump_to
            );

            fs::create_dir_all(dump_to.parent().unwrap())?;

            if e.src.is_file() || e.src.is_symlink() {
                let _ = fs::copy(&e.src, &dump_to)?;
                e.rm_src_file(ctx)?;
            } else if e.src.is_dir() {
                fs_extra::dir::copy(
                    &e.src,
                    &dump_to,
                    &fs_extra::dir::CopyOptions::new()
                        .copy_inside(false)
                        .content_only(true),
                )?;
                e.rm_src_dir_all(ctx)?;
            } else {
                return Err(anyhow!(
                    "cannot handle this type of file or whatever: {:?}",
                    &e.src
                ));
            }
            println!();

            e.symlink_to_src(ctx)?;
        }

        Ok(())
    }

    pub fn unlink_all(&self, ignore_non_links: bool, ctx: &Ctx) -> Result<()> {
        for e in
            self.home_entries
                .iter()
                .map(|p| self.entry_from_relative(&RelativePath::Home(p.to_path_buf()), ctx))
                .chain(self.non_home_entries.iter().map(|p| {
                    self.entry_from_relative(&RelativePath::NonHome(p.to_path_buf()), ctx)
                }))
        {
            if !e.src.is_symlink() || e.src.canonicalize()? != e.dest {
                if ignore_non_links {
                    continue;
                } else {
                    return Err(anyhow!("bad Entry: {:?}.", &e.relative));
                }
            }

            println!("deleting symlink: {:?}\n", &e.src);
            e.rm_src_file(ctx)?;
        }

        Ok(())
    }

    pub fn resolve_path(&self, path: impl AsRef<str>, ctx: &Ctx) -> Result<PathBuf> {
        let filename = PathBuf::from(
            shellexpand::tilde_with_context(path.as_ref(), || {
                Some(ctx.canon_home_dir.to_string_lossy())
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

    pub fn entry_from_dest(
        &self,
        dest: impl AsRef<Path>,
        ctx: &Ctx,
    ) -> Result<Entry, PathResolutionError> {
        let dest = dest.as_ref();

        if !dest.starts_with(&ctx.canon_repo) {
            return Err(PathResolutionError::OutsideRepo);
        }

        let relative = dest.strip_prefix(&self.module_dir).unwrap();
        let (src, relative) = match relative.starts_with(HOME) {
            true => {
                let stripped = relative.strip_prefix(HOME).unwrap().to_path_buf();
                (
                    ctx.canon_home_dir.join(&stripped),
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

    pub fn entry_from_src(
        &self,
        src: impl AsRef<Path>,
        ctx: &Ctx,
    ) -> Result<Entry, PathResolutionError> {
        let src = src.as_ref();

        if src.starts_with(&ctx.canon_repo) {
            return Err(PathResolutionError::InRepo);
        }

        let (dest, relative) = match src.starts_with(&ctx.canon_home_dir) {
            true => {
                let stripped = src.strip_prefix(&ctx.canon_home_dir).unwrap();
                (
                    self.module_dir.join(HOME).join(stripped),
                    RelativePath::Home(stripped.to_path_buf()),
                )
            }
            false => {
                let stripped = src.strip_prefix("/").expect("path must be absolute");
                (
                    self.module_dir.join(stripped),
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

    pub fn entry_from_relative(&self, rel: &RelativePath, ctx: &Ctx) -> Entry {
        match rel {
            RelativePath::Home(p) => Entry {
                src: ctx.canon_home_dir.join(p),
                relative: rel.clone(),
                dest: self.module_dir.join(HOME).join(p),
            },
            RelativePath::NonHome(p) => Entry {
                src: PathBuf::from("/").join(p),
                relative: rel.clone(),
                dest: self.module_dir.join(p),
            },
        }
    }

    pub fn entry(&self, path_str: impl AsRef<str>, ctx: &Ctx) -> Result<Entry> {
        let path = self.resolve_path(&path_str, ctx)?;
        match self.entry_from_dest(&path, ctx) {
            Ok(p) => Ok(p),
            Err(PathResolutionError::OutsideRepo) => match self.entry_from_src(&path, ctx) {
                Ok(p) => Ok(p),
                Err(_) => unreachable!(),
            },
            Err(_) => unreachable!(),
        }
    }

    pub fn add(&self, src: impl AsRef<str>, ctx: &Ctx) -> Result<()> {
        let src = src.as_ref();
        let e = match self.entry_from_src(self.resolve_path(src, ctx)?, ctx) {
            Ok(e) => e,
            Err(PathResolutionError::InRepo) => {
                println!("the path {} is already in the repo.", src);
                return Ok(());
            }
            Err(PathResolutionError::OutsideRepo) => unreachable!(),
        };

        let mut p = self.module_dir.clone();
        for c in e.relative.clone().relative().parent().unwrap().components() {
            let std::path::Component::Normal(c) = c else {
                unreachable!()
            };
            p.push(format!(".{}.{STUB}", c.to_str().unwrap()));

            if p.exists() {
                return Err(anyhow!(
                    "path is already in a directory managed by configma\n  src: {}\n  dir: {:?}\n",
                    src,
                    p,
                ));
            }
            p.pop();
            p.push(c);
        }

        if self.contains(&e) {
            println!("path is already maintained by configma: {}\n", src);
            return Ok(());
        }

        println!("moving path\n  src: {:?}\n  dst: {:?}\n", &e.src, &e.dest);

        fs::create_dir_all(e.dest.parent().unwrap())?;

        if e.src.is_file() {
            // the files should have read permissions without root
            fs::copy(&e.src, &e.dest)?;
            e.rm_src_file(ctx)?;
        } else if e.src.is_dir() {
            // the files should have read permissions without root
            fs_extra::dir::copy(
                &e.src,
                &e.dest,
                &fs_extra::dir::CopyOptions::new()
                    .copy_inside(false)
                    .content_only(true),
            )?;
            e.rm_src_dir_all(ctx)?;
            let _ = fs::File::create(
                e.dest
                    .parent()
                    .expect("path cannot be root")
                    .join(format!(".{}.{STUB}", e.dest.name())),
            )?;
        } else {
            return Err(anyhow!(
                "cannot handle this type of file or whatever: {}",
                &src
            ));
        }

        e.symlink_to_src(ctx)?;

        Ok(())
    }

    pub fn remove(&self, src: impl AsRef<str>, ctx: &Ctx) -> Result<()> {
        let src = src.as_ref();
        let e = self.entry(src, ctx)?;
        if self.contains(&e) {
            println!("restoring path\n  src: {:?}\n  dst: {:?}\n", e.src, e.dest,);

            e.rm_src_file(ctx)?;
            if e.dest.is_dir() {
                fs::remove_file(
                    e.dest
                        .parent()
                        .expect("path cannot be root")
                        .join(format!(".{}.{STUB}", e.dest.name())),
                )?;
                e.copy_dir_to_src(ctx)?;
                fs::remove_dir_all(&e.dest)?;
            } else if e.dest.is_file() {
                e.copy_file_to_src(ctx)?;
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
                let p = self.module_dir.join(&parent);
                if p.read_dir()?.count() == 0 {
                    fs::remove_dir(p)?;
                } else {
                    break;
                }
            }
        } else {
            return Err(anyhow!("file '{}' not in module '{}'", &src, self.name));
        }
        Ok(())
    }
}
