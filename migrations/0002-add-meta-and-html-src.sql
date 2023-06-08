alter table links add column meta text default(null);
alter table links add column src blob default(null);
alter table links add column last_fetched int default(null);
alter table links add column last_processed int default(null);
alter table links add column http_headers blob default(null);
