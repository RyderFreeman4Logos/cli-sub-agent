To fix the issue of ephemeral sessions breaking gemini-cli file access due to the wrong working directory, you can modify the `csa run` command to set the working directory to the project root. 

Here's the exact code fix:

```bash
cat <<'PROMPT' | csa run --sa-mode true --force-ignore-tier-setting --tool gemini-cli --idle-timeout 300 --ephemeral --cd /path/to/project/root
Read crates/mandate-core/crates/mandate-bounty/fixtures/bounty.toml and report its contents.
PROMPT
```

Replace `/path/to/project/root` with the actual path to your project root directory.

Alternatively, you can also set the `cwd` in the tool invocation to the project root:

```bash
cat <<'PROMPT' | csa run --sa-mode true --force-ignore-tier-setting --tool gemini-cli --idle-timeout 300 --ephemeral --cwd /path/to/project/root
Read crates/mandate-core/crates/mandate-bounty/fixtures/bounty.toml and report its contents.
PROMPT
```

Or, you can inject the project root path in the prompt context:

```bash
cat <<'PROMPT' | csa run --sa-mode true --force-ignore-tier-setting --tool gemini-cli --idle-timeout 300 --ephemeral
Set the working directory to /path/to/project/root and read crates/mandate-core/crates/mandate-bounty/fixtures/bounty.toml and report its contents.
PROMPT
```

In the `csa` code, you can modify the `run` function to set the working directory to the project root when `--ephemeral` is used:

```python
if args.ephemeral:
    # Set the working directory to the project root
    os.chdir('/path/to/project/root')
```

Or, you can add a new option `--project-root` to specify the project root directory:

```python
parser.add_argument('--project-root', help='Path to the project root directory')
```

Then, in the `run` function, you can set the working directory to the project root:

```python
if args.project_root:
    os.chdir(args.project_root)
```