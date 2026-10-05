-- Ограничение перебора паролей (SPEC-01, ADR-016).
create table login_attempts (
    login          text primary key,
    failures       integer not null,
    last_failed_at timestamptz not null,
    locked_until   timestamptz
);
