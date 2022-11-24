create table if not exists "friends" (
  id integer primary key asc autoincrement,
  name text not null unique on conflict rollback,
  url text not null
);

create table if not exists "links" (
  id integer primary key asc autoincrement,
  url text not null unique on conflict rollback,
  title text not null,
  tags text not null default(''),
  via text default(null),
  notes text default(null),
  found_at integer(8) default(null),
  read_at integer(8) default(null),
  published_at integer(8) default(null),
  from_filename text default(null)
);
