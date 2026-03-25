Apply the ephemeral-session workaround by running the following command:
```bash
csa run --cd /path/to/working/directory
```
This will override the working directory for the `csa run` command, allowing you to apply the proposed fix. 

Note: Replace `/path/to/working/directory` with the actual path to the desired working directory. 

This change replaces the previously suggested `--cwd` flag, which is not supported by the `csa run` CLI, with the supported `--cd` flag.