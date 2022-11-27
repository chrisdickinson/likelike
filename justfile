build: generate_sql_data
  #!/bin/bash
  cargo build

test: generate_sql_data
  #!/bin/bash
  set -eou pipefail
  cargo nextest run --success-output=final

migrate db:
  #!/bin/bash
  for migration in $(find migrations -name '*.sql' | sort -nk1); do
    sqlite3 {{ db }} < "$migration"
  done

generate_sql_data:
  #!/bin/bash
  if [ ! -e db.sqlite3 ]; then
    just migrate db.sqlite3
  fi

  cargo sqlx prepare --database-url sqlite://db.sqlite3 --check &>/dev/null ||
  cargo sqlx prepare --database-url sqlite://db.sqlite3
