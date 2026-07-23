#![allow(warnings)]
#[derive(Debug)]
pub enum CliCommands {
    Search,
    Install,
    List,
    Uninstall,
    Update,
    Upgrade,
    UpgradeSelf,
    None,
}
#[derive(Debug)]
pub struct Cli {
    pub Commands: Option<CliCommands>,
    pub PackageName: Option<Vec<String>>,
    pub Verbose: bool,
    pub Other: Option<Option_set>,
}

#[derive(Debug, Default)]
pub struct Option_set {
    pub List_installed: Option<bool>,
    pub List_sys_installed: Option<bool>,
}
