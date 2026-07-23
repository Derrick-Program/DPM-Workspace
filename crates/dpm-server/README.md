# DPM-Server
DPM-Server is a server for the DPM-Client. It is a simple server that can be used to store and retrieve files from the DPM-Client.

## Installation
To install the server, you need to have the following installed:
- cargo (Rust)
```
cargo install DPM-Server
```

## Useage
To use the server, you need to run the following command:
- ```dpm-server init``` This will initialize the Package and create a new directory for the Project.
- ```dpm-server hash``` This will hash the files in the directory and store them in the PackageInfo.json.
- ```dpm-server build``` This will build the Project.
- ```dpm-server fix``` This will fix the RepoInfo.json.
