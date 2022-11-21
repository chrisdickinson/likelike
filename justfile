build: generate_sql_data
  #!/bin/bash
  cargo build

test: generate_sql_data
  #!/bin/bash
  set -eou pipefail
  cargo nextest run --success-output=final

generate_sql_data:
  #!/bin/bash
  if [ ! -e db.sqlite3 ]; then
    for migration in $(find migrations -name '*.sql' | sort -nk1); do
      sqlite3 db.sqlite3 < "$migration"
    done
  fi

  cargo sqlx prepare --database-url sqlite://db.sqlite3 --check &>/dev/null ||
  cargo sqlx prepare --database-url sqlite://db.sqlite3
