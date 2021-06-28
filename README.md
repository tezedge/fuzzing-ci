# Fuzzing CI

This program implements GitHub webhook to start or restart fuzz tests on a
project as a new commit arrives.

It might be useful for projects that use fuzz testing (especially using
`honggfuzz`) as a part of their QA.

It can use a dedicated directory for fuzzing corpora (input files for fuzz targets
that give different coverage feedback), so after restart previous coverage will
be achieved pretty soon.

This program can generate coverage reports (using `kcov` utility) so it is
possible to see line-based coverage provided by the corpora files. 

## Requirements

- Git utility to check out fuzzed project
- Rust toolchain that is used for building fuzzed project

## Building

```
cargo build
```

## Configuration

The most of configuration parameters for the program should be specified via a
TOML configuration file (see below for details). The `-c/--config` option tells
it what file to use. By default, the `fuzz-ci.toml` file from the current
directory is used. See the [fuzz-ci.toml](fuzz-ci.toml) for description on all parameters.

### Fuzzing Project

This CI uses a shell script to check out both fuzzing project (the one that
defines fuzz targets) and the target project (the one defining functions being
tested).

The current implementation of the [checkout.sh](checkout.sh) implies that the target project
is a submodule of the fuzzing project.

### Reports

The `reports_path` configuration element is used to specify the directory where
reports will be placed to. Also it is possible to specify the `url` parameter
with an externally accessible URL which allows accessing that directory via
HTTP.

``` toml
# Path to put coverage reports to
reports_path = "../reports"

# Url for the reports
url = "http://reports.example.com/"
```

### Slack Integration

The fuzzing CI can provide feedback via a Slack channel so persons subscribed to
that channel will be notified on fuzzing stages and events.

As prerequisites, a Slack application associated with the CI should be added to
the team. Then, a separate channel should be created for fuzzing events, and
this application should be configured to be allowed to post messages in that
channel. See [here](https://api.slack.com/authentication/basics) for more details.

After that, both the application authentication token and the channel ID should
be specified in the configuration file:

``` toml
[slack]
channel = "XXXXXXX"
token = "xoxb-XXXXXXXXXXX..."
```

It is also possible to use environment variable `SLACK_AUTH_TOKEN` to avoid
specifying the token in the configuration file.

``` sh
SLACK_AUTH_TOKEN="xoxb-XXX....." fuzzing-ci server
```

By default, only errors and timeouts are reported to the slack channel. To
enable other events (like starting of fuzzing, coverage update etc.) the
configuration key `verbose` should be set to `true`:

``` toml
[slack]
channel = "XXXXXXX"
token = "xoxb-XXXXXXXXXXX..."
verbose = true
```


### Configuration Sample

The [samples/fuzz-ci.toml](samples/fuzz-ci.toml) is a sample configuration with description for each parameter

## Running

``` sh
fuzzing-ci
```

To run the program as a webhook so it will be notified on pushes, the `server`
subcommand should be used.

``` sh
fuzzing-ci server
```

## Configuring GitHub Webhook

To receive notifications from GitHub, a webhook should be added to the
repository that we need to listen to (note that it might be a separate from the
repository containing the fuzzing projects).

Open the target project repository settings, select *Webhooks* item and press
*Add webhook*.

In the *Payload URL* enter the URL the app is accessible with, adding `/run` path
(e.g. http::/example.com:3030/run).

In the *Content type* select *application/json*.

Press *Add webhook*, and you're set.

## Testing Installation

Commit a change to the branch the CI is configured for and push it to the
repository with the webhook configured. The fuzzing will be started soon. 
