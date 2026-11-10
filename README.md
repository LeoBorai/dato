# dato

A database CLI with completion, syntax highlighting and several table formats, inspired by
[litecli](https://github.com/dbcli/litecli), pgcli and mycli. SQLite is supported today;
Postgres and MySQL are planned.

## Install

```sh
cargo install dato
```

## Usage

```sh
dato app.db                                 # interactive shell; creates app.db if missing
dato                                        # in-memory database
dato app.db -e "select * from users;"       # run SQL and exit
echo "select 1 as n;" | dato :memory:       # run SQL from stdin
dato app.db -f csv -e "select * from t;"    # choose the output format
```

When output is piped, the default format is `tsv`; on a terminal it is `ascii`.
In batch mode the first failing statement stops the run with exit status 1.

## Shell commands

| Command | Description |
|---|---|
| `.tables [prefix]`, `\dt` | List tables and views |
| `.views [prefix]`, `\dv` | List views |
| `.schema [table]` | Show `CREATE` statements |
| `.indexes [table]`, `\di` | List indexes |
| `.databases`, `\l` | List attached databases |
| `\d [table]`, `describe` | Describe a table's columns |
| `\T [format]` | Show or set the output format |
| `<sql>\G` | Run a query in vertical format |
| `help`, `\?` | List commands |
| `quit`, `exit`, `\q` | Leave the shell |

## Output formats

`ascii`, `psql`, `markdown`, `csv`, `tsv`, `json`, `vertical`.

## Key bindings

| Key | Action |
|---|---|
| Enter | Submit when the statement ends with `;`, otherwise add a line |
| Alt-Enter | Submit regardless |
| Tab | Complete keywords, tables, columns and commands |
| Ctrl-O, or end the input with `\e` | Edit in `$VISUAL` / `$EDITOR` (default `vi`) |
| Ctrl-C | Cancel a running query, or clear the input |
| Ctrl-D | Exit |

History is saved in the platform data directory (`dato/history`).

## Development

```sh
cargo install cargo-nextest --locked
just test
```

## License

MIT OR Apache-2.0
