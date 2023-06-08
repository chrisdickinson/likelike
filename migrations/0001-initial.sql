create table if not exists "friends" (
  id integer primary key asc autoincrement,
  name text not null unique on conflict rollback,
  url text not null
) strict;

create table if not exists "links" (
  id integer primary key asc autoincrement,
  url text not null unique on conflict rollback,
  title text default(null),
  tags text not null default(''),
  via text default(null),
  notes text default(null),
  found_at int default(null),
  read_at int default(null),
  published_at integer default(null),
  from_filename text default(null),
  image text default(null)
) strict;

create table if not exists "database_version" (
  id integer default(0) primary key check (id = 0),
  version int default 0
) strict;
