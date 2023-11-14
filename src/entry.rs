use std::{
    collections::HashSet,
    fs,
    os::unix::{self, prelude::MetadataExt},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use nix::unistd;
use users::User;

use crate::config::Ctx;

pub const STUB: &str = ".configma.stub";
pub const HOME: &str = "home";

#[derive(Debug)]
pub struct Privilege<'a> {
    pub ctx: &'a Ctx,
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
pub enum RelativePath {
    Home(PathBuf),
    NonHome(PathBuf),
}

impl RelativePath {
    pub fn relative(self) -> PathBuf {
        match self {
            RelativePath::Home(p) => PathBuf::from(HOME).join(p),
            RelativePath::NonHome(p) => p,
        }
    }

    pub fn path(&self) -> &Path {
        match &self {
            RelativePath::Home(p) => p,
            RelativePath::NonHome(p) => p,
        }
    }
}

#[derive(Debug)]
pub struct Entry {
    pub src: PathBuf,
    pub relative: RelativePath,
    pub dest: PathBuf,
}

impl Entry {
    pub fn get_priv<'a>(&self, ctx: &'a Ctx) -> Result<Option<Privilege<'a>>> {
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

    pub fn dump(&self, ctx: &Ctx) -> Result<()> {
        let dump_to = ctx.dump_dir.join(self.relative.clone().relative());
        fs::create_dir_all(dump_to.parent().unwrap())?;

        let src_meta = self.src.parent().expect("must have a parent").metadata()?;
        let dest_meta = dump_to.parent().expect("must have a parent").metadata()?;
        let same_dev = src_meta.dev() == dest_meta.dev();

        let p = self.get_priv(ctx)?;
        if self.src.is_file() || self.src.is_symlink() {
            if self.src.is_symlink() {
                let to = fs::read_link(&self.src)?;
                unix::fs::symlink(to, &dump_to)?;
                fs::remove_file(&self.src)?;
            } else if same_dev {
                fs::rename(&self.src, &dump_to)?;
                if p.is_some() {
                    chown_recursive(&dump_to, ctx.non_root_user.clone())?;
                }
            } else {
                match fs::copy(&self.src, &dump_to) {
                    Ok(_) => (),
                    Err(err) => {
                        if dump_to.exists() {
                            let _ = fs::remove_file(dump_to);
                        }
                        return Err(err)?;
                    }
                }
                fs::remove_file(&self.src)?;
                if p.is_some() {
                    chown_recursive(&dump_to, ctx.non_root_user.clone())?;
                }
            }
        } else if self.src.is_dir() {
            if same_dev {
                fs::rename(&self.src, &dump_to)?;
            } else {
                match fs_extra::dir::copy(
                    &self.src,
                    &dump_to,
                    &fs_extra::dir::CopyOptions::new()
                        .copy_inside(false)
                        .content_only(true),
                ) {
                    Ok(_) => (),
                    Err(err) => {
                        if dump_to.exists() {
                            let _ = fs::remove_dir_all(&dump_to);
                        }
                        return Err(err)?;
                    }
                }
                fs::remove_dir_all(&self.src)?;
            }
            if p.is_some() {
                chown_recursive(&dump_to, ctx.non_root_user.clone())?;
            }
        } else {
            return Err(anyhow!(
                "cannot handle this type of file or whatever: {:?}",
                &self.src
            ));
        }
        unix::fs::symlink(&self.dest, &self.src)?;

        drop(p);
        Ok(())
    }

    pub fn add(&self, ctx: &Ctx) -> Result<()> {
        fs::create_dir_all(self.dest.parent().unwrap())?;

        let src_meta = self.src.parent().expect("must have a parent").metadata()?;
        let dest_meta = self.dest.parent().expect("must have a parent").metadata()?;
        let same_dev = src_meta.dev() == dest_meta.dev();

        let p = self.get_priv(ctx)?;
        if self.src.is_file() {
            if same_dev {
                fs::rename(&self.src, &self.dest)?;
            } else {
                match fs::copy(&self.src, &self.dest) {
                    Ok(_) => (),
                    Err(err) => {
                        if self.dest.exists() {
                            let _ = fs::remove_file(&self.dest);
                        }
                        return Err(err)?;
                    }
                }
                fs::remove_file(&self.src)?;
            }
        } else if self.src.is_dir() {
            if same_dev {
                fs::rename(&self.src, &self.dest)?;
            } else {
                match fs_extra::dir::copy(
                    &self.src,
                    &self.dest,
                    &fs_extra::dir::CopyOptions::new()
                        .copy_inside(false)
                        .content_only(true),
                ) {
                    Ok(_) => (),
                    Err(err) => {
                        if self.dest.exists() {
                            let _ = fs::remove_dir_all(&self.dest);
                        }
                        return Err(err)?;
                    }
                }
                fs::remove_dir_all(&self.src)?;
            }
            let _ = fs::File::create(self.dest.join(STUB))?;
        } else {
            return Err(anyhow!(
                "cannot handle this type of file or whatever: {:?}",
                &self.src
            ));
        }
        if p.is_some() {
            chown_recursive(&self.dest, ctx.non_root_user.clone())?;
        }
        unix::fs::symlink(&self.dest, &self.src)?;
        drop(p);

        Ok(())
    }

    pub fn remove(&self, ctx: &Ctx) -> Result<()> {
        let src_meta = self.src.parent().expect("must have a parent").metadata()?;
        let dest_meta = self.dest.parent().expect("must have a parent").metadata()?;
        let same_dev = src_meta.dev() == dest_meta.dev();

        let p = self.get_priv(ctx)?;
        fs::remove_file(&self.src)?;
        if self.dest.is_dir() {
            fs::remove_file(self.dest.join(STUB))?;
            if same_dev {
                fs::rename(&self.dest, &self.src)?;
            } else {
                match fs_extra::dir::copy(
                    &self.dest,
                    &self.src,
                    &fs_extra::dir::CopyOptions::new()
                        .copy_inside(false)
                        .content_only(true),
                ) {
                    Ok(_) => (),
                    Err(err) => {
                        if self.src.exists() {
                            let _ = fs::remove_dir_all(&self.src);
                        }
                        let _ = unix::fs::symlink(&self.dest, &self.src);
                        return Err(err)?;
                    }
                }
                fs::remove_dir_all(&self.dest)?;
            }
        } else if self.dest.is_file() {
            if same_dev {
                fs::rename(&self.dest, &self.src)?;
            } else {
                match fs::copy(&self.dest, &self.src) {
                    Ok(_) => (),
                    Err(err) => {
                        if self.src.exists() {
                            let _ = fs::remove_file(&self.src);
                        }
                        let _ = unix::fs::symlink(&self.dest, &self.src);
                        return Err(err)?;
                    }
                }
                fs::remove_file(&self.dest)?;
            }
        } else {
            return Err(anyhow!(
                "cannot handle this type of file or whatever: {:?}",
                &self.src
            ));
        }
        if p.is_some() {
            chown_recursive(&self.src, ctx.root_user.clone().unwrap())?;
        }
        drop(p);
        Ok(())
    }

    pub fn rm_src_file(&self, ctx: &Ctx) -> Result<()> {
        let p = self.get_priv(ctx)?;

        fs::remove_file(&self.src)?;

        drop(p);
        Ok(())
    }

    pub fn symlink_to_src(&self, ctx: &Ctx) -> Result<()> {
        let p = self.get_priv(ctx)?;

        unix::fs::symlink(&self.dest, &self.src)?;

        drop(p);
        Ok(())
    }
}

fn chown_recursive(path: impl AsRef<Path>, user: User) -> Result<()> {
    let path = path.as_ref();
    let uid = Some(unistd::Uid::from(user.uid()));
    let gid = Some(unistd::Gid::from(user.primary_group_id()));
    if path.is_file() {
        nix::unistd::chown(path, uid, gid)?;
    } else if path.is_symlink() {
        let to = fs::read_link(path)?;
        fs::remove_file(path)?;
        unix::fs::symlink(to, path)?;
    } else if path.is_dir() {
        let mut paths = vec![path.to_path_buf()];
        while let Some(path) = paths.pop() {
            for e in fs::read_dir(&path)? {
                let e = e?;
                let ft = e.file_type()?;
                let p = e.path();

                if ft.is_file() {
                    nix::unistd::chown(&p, uid, gid)?;
                } else if ft.is_dir() {
                    nix::unistd::chown(&p, uid, gid)?;
                    paths.push(p);
                } else if ft.is_symlink() {
                    let to = fs::read_link(&p)?;
                    fs::remove_file(&p)?;
                    unix::fs::symlink(to, &p)?;
                } else {
                    return Err(anyhow!("can't handle this type of path: {:?}", path));
                }
            }
        }
    } else {
        return Err(anyhow!("can't handle this type of path: {:?}", path));
    }
    Ok(())
}

pub fn generate_entry_set(parent_dir: impl AsRef<Path>) -> Result<HashSet<PathBuf>> {
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
                    if p.join(STUB).exists() {
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

pub trait Convenience {
    fn name(&self) -> &str;
}

impl Convenience for &Path {
    fn name(&self) -> &str {
        self.file_name()
            .expect("no file name on file")
            .to_str()
            .expect("non utf string?")
    }
}
impl Convenience for PathBuf {
    fn name(&self) -> &str {
        self.file_name()
            .expect("no file name on file")
            .to_str()
            .expect("non utf string?")
    }
}
