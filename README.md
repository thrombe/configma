# Configma
a very simple config files manager.

# Working
very simple.
very small codebase.
uses with git repository for convinience.
`configma add <file / dir>` moves the file / dir into the repo directory mentioned in the config file for configma.
there is no separate database for configma. it uses the files and directories in the repo to symlink the files / dirs at the correct place.
directories contain a stub file that helps configma know that the directory must be symlinked.
configma supports multiple profiles. you can switch between profiles really easily (`configma switch-profile <profile name>`).
you can use -f flag to force sync / apply a config profile. it moves the current configs in a temp directory so that it is not lost is any accidents happen.
you can remove files from configma really easily. `configma remove <path>` path can be a path to the file in your configs or a path in the repo. configma handles it correctly. this command restores the files / dirs from the repo to the correct place in your system.


# how to use
`git clone <this repo>`
`cd configma`
`cargo install --path .`

## create a new profile
`mkdir -p ~/.config/configma`
`echo 'repo = <some path that you want configma to store your config files in>' > ~/.config/configma/config.toml`
`configma new-profile <profile name>`

## switch to the profile
`configma switch-profile <profile name>`

## add file / dir to configma
`configma add <path>`

## remove / restore a file from configma
`configma remove <path>`

## sync changes from repo to the system
`configma sync`

## todo
- some kinds template / inheritance system to have profiles inherit from other profiles
- git integration?


