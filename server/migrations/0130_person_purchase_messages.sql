-- Задача 053: сообщения о покупке частным клиентам. О чеках до установки не пишем.
insert into bot_state (key, value)
values ('client_messages_person_since', to_jsonb(to_char(now() at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')))
on conflict (key) do nothing;
