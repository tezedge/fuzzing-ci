# Fuzzing CI

This program is designed to run fuzzing on selected branches of a source
project, restarting it as a new commit arrives.

## Building

```
cargo build
```

## Running

The most of configuration parameters for the program should be specified via a
TOML configuration file (see below for details). The `-c/--config` option tells
it what file to use.

To run the program as a webhook so it will be notified on pushes, the `server`
subcommand should be used. It is also possible to specify a URL that allows to
view coverage reports using the `-u/--url` parameter. If you need it to update
the team via a Slack channel, an access token might be specified via an
environment variable `SLACK_AUTH_TOKEN` (see below for Slack integration
details)

```
SLACK_AUTH_TOKEN="xoxb-XXXXXXX" cargo run -- -c config.toml server --url http://fuzz.example.com
```

## HTTP Endpoints

The application exposes two endpoints:

- '/api' to work as a GitHub webhook.
- '/reports' to serve static content of Kcov-generated reports.

## Configuration

The program is controlled by a TOML configuration file. 

See the [config.toml] file for the details on possible parameters.

## Webhook Configuration

To receive notifications from GitHub, a webhook should be added to the
repository that we need to listen to (note that it might be a separate from the
repository containing the fuzzing projects).

Open the repository settings, select *Webhooks* item and press *Add webhook*.

In the *Payload URL* enter the URL the app is accessible with, with `/api` path
(e.g. http::/example.com:3030/run).

In the *Content type* select *application/json*.

Press *Add webhook*, and you're set.

## Nginx Configuration

A webserver might be configured to display Kcov reports. If that is Nginx, the
[sample configuration file](samples/nginx/fuzzing-ci.conf) can be used.

## Slack Integration

A Slack app should be created to interact with a channel, see
[here](https://api.slack.com/start/overview#creating). After the Slack
application is created, its OAuth token should be specified via the
`slack.token` configuration key or via `SLACK_AUTH_TOKEN` environment variable.

A Slack channel should be specified via `slack.channel` parameter.

## Debugging

By default the program uses `info` logging level. Adding a single `-d` parameter
turns on `debug` level logging, and another one `-d` parameter makes `trace`
logging visible (only for `debug` builds).

```
cargo run -- -dd ...
```

## Implementation Details

The application implements a GitHub webhook (currently only `ping` and `push`
events). On each `push` event for the specified branch it starts fuzzing cycle
for the head version of that branch.

### Fuzzing Cycle

First, if the branch (its previous version) is in the process of fuzzing, all
the fuzzers are stopped.

Then, the program checks out the fuzzing project. Currently this is done by
running the script [checkout.sh].

After the fuzzing project is checked  out, its fuzzing projects are prepared for
fuzzing:
- Kcov is run against the project, with fuzzing corpus as input, to get source
  coverage given by the corpus files.
- Fuzzing project is built by running `cargo hfuzz build`.
- Fuzzer is started for each of the fuzzing targets by launching `cargo hfuzz run <target>`.

### Fuzzing Feedback

Currently only Honggfuzz is supported. It provides very nice feedback for a
human user, but to make convertable to any different presentation some tricks
are needed.

When `honggfuzz` is run with `-v` switch, it does not show that term feedback,
but instead reports progress in the following form:

```

```

The first group of `/`-separated numbers are: ...

For us the most valuable number is the cound of covered edges.

Also by running the `honggfuzz` on a target shortly and then stopping it (e.g.
by specifying a low number of iterations or a short period of time) we can see
the total number of edges detected by it. So using that number and collecting
covered edges as the fuzzing goes we can report current progress in the form of
*covered*/*total* edges.

