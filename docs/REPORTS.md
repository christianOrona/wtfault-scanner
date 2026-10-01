# Problem reports

The app can assemble a problem report: its version, the operating system, how
the last run ended and the end of its log. A person can copy it, save it, or,
in a build that has a report address, press **Send** (#50).

## What Send does

- It posts the text **exactly as shown on screen**, with the app's version and
  the operating system, and nothing else.
- Identifiers are withheld by default, before the text is shown: every VIN the
  app has recorded and anything else shaped like a VIN with a valid check digit
  is reduced to its manufacturer and model year, and the user name and home
  folder are taken out of file paths. The screen says what was taken out, and
  the person can turn it off.
- It is never automatic. There is no setting that sends reports for you.
- If the address cannot be reached, the report can still be copied or saved,
  exactly as before.

## Giving a build an address

The address is compiled in when the project builds a release:

```sh
AIM_REPORT_ENDPOINT=https://reports.example.org cargo build --release -p aim-api
```

It can be overridden when the app starts, by the same variable, which is how a
self-hosted endpoint or a test points the app elsewhere. Only `https://`
addresses are accepted, apart from `http://127.0.0.1` and `http://localhost`.
A build with no address shows no Send button.

## Running the endpoint

`apps/report-endpoint` is the receiving end. It needs no database:

```sh
cargo run --release -p aim-report-endpoint -- --dir /var/lib/wtfault-reports --port 8790
```

Put it behind a proxy that terminates TLS, and pass `--trust-forwarded-for`
only then. What it promises:

| | |
|---|---|
| Body | At most 256 KiB of report text. Larger is refused before it is parsed. |
| Rate | 5 reports per address per hour, and 1,000 per day across everybody (`--per-address`, `--per-day`). |
| Storage | One text file per report under a folder per day: the text, version, platform and time of arrival. The sender's address is held in memory for the rate limit only. |
| Input | Only `version`, `platform` and `text` are accepted; anything else is refused. Nothing received is executed, fetched or used as a path. |

`GET /v1/health` answers `ok`. Reports arrive at `POST /v1/reports`.
