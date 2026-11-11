# dato

Database CLI for your Terminal

## Install

Prebuilt binary (Linux, macOS):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/LeoBorai/dato/main/install.sh | sh
```

Set `DATO_VERSION` (e.g. `v0.1.0`) or `DATO_INSTALL_DIR` (default `~/.local/bin`) to override. Windows: download the `.zip` from the releases page.

From source:

```sh
cargo install dato
```

## Usage

```sh
dato app.db                                 # SQLite shell; creates app.db if missing
dato sqlite app.db                          # same, with the explicit subcommand
dato :memory:                               # in-memory SQLite database
dato mysql -h db.local -u app -p shop       # MySQL shell; asks for the password
dato psql -h db.local -U app -W shop        # PostgreSQL shell; asks for the password
dato                                        # shell without a connection; use \connect
dato app.db -e "select * from users;"       # run SQL and exit
echo "select 1 as n;" | dato :memory:       # run SQL from stdin
dato app.db -T csv -e "select * from t;"    # choose the output format (--format)
```

When output is piped, the default format is `tsv`; on a terminal it is `ascii`.
In batch mode the first failing statement stops the run with exit status 1.
`-e` and piped input need a database target.

### Servers

| Server | Arguments |
|---|---|
| `sqlite` | `[FILE]`: database file, or `:memory:` (default) |
| `mysql` | `-h/--host` (default `localhost`), `-P/--port` (default `3306`), `-u/--user` (default: OS user), `-p/--password` (prompt, no echo), `-S/--socket`, `-D/--database` or `DB` |
| `psql` | `-h/--host` (default `localhost`, or a socket directory), `-p/--port` (default `5432`), `-U/--username` (default: OS user), `-d/--dbname` or `DB` (default: the user name), `-W/--password` (prompt, no echo), `--sslmode`, `--sslrootcert` |

Each server has its own arguments, so a letter can mean different things: `-P` is the
MySQL port, `-p` the PostgreSQL one. Login arguments follow the `mysql` and `psql`
clients. Use `dato <server> --help` for help: `-h` is the host.

### PostgreSQL and TLS

`--sslmode` takes the same values as libpq, default `prefer`:

| Mode | Behavior |
|---|---|
| `disable` | Never use TLS |
| `prefer` | Use TLS when the server offers it, else connect unencrypted |
| `require` | Require TLS; the certificate is not checked |
| `verify-full` | Require TLS with a certificate that chains to a trusted root and matches the host name |

`verify-full` trusts the system roots, or the PEM file given as `--sslrootcert`.
`prefer` can be downgraded by someone on the network path; use `require` or
`verify-full` when that matters.

Compared with `psql`: only the shell commands below, not psql's full set of backslash
meta-commands, and no `PGPASSWORD`, `.pgpass` or connection strings.
A PostgreSQL connection is bound to one database: switch with `\connect psql -U app other`.
`.schema` rebuilds `CREATE TABLE` from the catalog (columns, defaults, constraints); it
is not `pg_dump`.

### Connecting from the shell

`\connect` opens a wizard: pick a server with the arrow keys, then fill in its fields
(the password is masked). `\connect <server> <args>` connects directly with the same
arguments as the CLI, e.g. `\connect mysql -u root -p shop`, `\connect psql -U app -W shop`
or `\connect sqlite other.db`.
A successful connect replaces the current connection; a failed one keeps it.

### Migrating from 0.0.1

- `-f FORMAT` is now `-T FORMAT` / `--format FORMAT`.
- `dato` with no target starts disconnected instead of opening an in-memory database;
  use `dato :memory:` for the old behavior.
- URL targets are not accepted: use `dato mysql -h HOST -u USER -p DB` or
  `dato psql -h HOST -U USER -W DB` instead of `mysql://` and `postgres://` URLs.

## Shell commands

| Command | Description |
|---|---|
| `.tables [prefix]`, `\dt` | List tables and views |
| `.views [prefix]`, `\dv` | List views |
| `.schema [table]` | Show `CREATE` statements |
| `.indexes [table]`, `\di` | List indexes |
| `.databases`, `\l` | List databases (SQLite: attached; MySQL: schemas; PostgreSQL: databases) |
| `\d [table]`, `describe` | Describe a table's columns |
| `\T [format]` | Show or set the output format |
| `\connect [server [args]]` | Connect to a database; without arguments, choose interactively |
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

Results wider than the terminal open in a viewer instead of wrapping:
Left/Right (or `h`/`l`) scroll 8 columns, Home/End jump to the edges,
Up/Down/PageUp/PageDown scroll rows of tall results, and `q`, Enter or Esc close it.
The last view stays on screen.

Application data lives in `~/.dato` (created with mode `0700` on Unix):

| File | Contents |
|---|---|
| `~/.dato/history` | Shell history (last 1000 entries) |
| `~/.dato/edit.sql`, `~/.dato/edit-*.sql` | Buffers for the external editor (Ctrl-O, `\e`) |

History from the previous location (`<data dir>/dato/history`) is copied to
`~/.dato/history` on first start; the old file is left in place.

## Development

```sh
cargo install cargo-nextest --locked
just test
```

The MySQL integration tests (`tests/mysql.rs`) run only when `DATO_MYSQL_URL` is set.
The user must be able to create and drop databases (each test uses its own `dato_test_*`):

```sh
docker run -d --name dato-mysql -e MYSQL_ROOT_PASSWORD=dato -p 3306:3306 mysql:8.4
DATO_MYSQL_URL=mysql://root:dato@127.0.0.1:3306 cargo nextest run --test mysql
```

The PostgreSQL integration tests (`tests/psql.rs`) run only when `DATO_PSQL_URL` is set,
with the same rule about creating and dropping databases:

```sh
docker run -d --name dato-pg -e POSTGRES_PASSWORD=dato -p 5432:5432 postgres:17
DATO_PSQL_URL=postgres://postgres:dato@127.0.0.1:5432 cargo nextest run --test psql
```

Against a server with TLS, also set `DATO_PSQL_CA` to the CA certificate file that signed the
server certificate (an end-entity certificate, not a self-signed CA); see the `psql` job in
`.github/workflows/ci.yml` for how CI generates one.

## License

MIT OR Apache-2.0
