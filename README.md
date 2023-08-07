# Configma
Configma is a powerful yet incredibly simple and efficient config files manager. Main focus is on simplicity, reliability, and ease of use.

# Features
- User-Friendly and Minimalistic: Configma is built with simplicity in mind. Its minimalistic design makes it easy to use, even for beginners.
- Easy Git Integration: Configma can be easily used with git for even more convinience.
- Small codebase: there is very little code in this project. Managing config files in a very complex way seems like an overkill. But manually managing them is a pain. So configma provides a nice middleground.
- Configma add: Adding files or directories to Configma is a breeze with `configma add <file / dir>`, which moves the specified file or directory into the repository directory mentioned in the Configma config file. The tool doesn't rely on a separate database; instead, it cleverly uses the files and directories in the repository to symlink them at the correct locations.
- Directory stub files: Configma uses a 'configma_dir.stub' file placed within symlinked directories to differentiate directories added to Configma from individual files. Using this approach, Configma avoids the need for a separate database, maintaining its lightweight design. 
- Configma remove: Removing files from Configma is just as straightforward with `configma remove <path>`. Whether the path points to a file within your configs or the repository, Configma handles it correctly. This command restores the files/directories from the repository to their original places in your system, making management effortless.
- Profiles: Profiles enable users to consolidate config files from various systems into a single repository. The currently applied profile name is stored in the '~/.config/configma/profile' file, allowing users to seamlessly switch between profiles on each system using the `configma switch-profile <name>` command.
- Force sync: Worried about data loss? The -f flag enables you to force sync or apply a config profile, moving your current configs to a temporary directory to safeguard against accidents.


# how to use
## Installation
Clone this repository, navigate to the Configma directory and Install Configma using Cargo.
```zsh
git clone https://github.com/thrombe/configma
cd configma
cargo install --path .
````

## Create a new profile
Set up a new profile by creating the Configma configuration file.
```zsh
mkdir -p ~/.config/configma
echo 'repo = <path>' > ~/.config/configma/config.toml
configma new-profile <profile name>
configma switch-profile <profile name>
```
the repo path is any directory where you would like configma to store your config files in.

## Switch Profiles
```zsh
configma switch-profile <profile name>
````

## Add files / directories to current profile
```zsh
configma add <path>
```

## Remove / Restore a file from current profile
Whether the path points to a file within your configs or the repository, Configma handles it correctly. This command restores the files/directories from the repository to their original places in your system.
```zsh
configma remove <path>
```

## Sync changes
Sync any changes made in the repo to the system.
```zsh
configma sync
```

# todo
- [ ] Template/Inheritance System: A template system that allows profiles to inherit configurations from other profiles to reduce redundancy.
- [ ] Git integration


