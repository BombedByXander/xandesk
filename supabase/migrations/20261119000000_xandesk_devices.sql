create table if not exists public.xandesk_devices (
  id text primary key check (id ~ '^[0-9a-f]{32}$'),
  owner_id uuid not null references auth.users(id) on delete cascade,
  room_id text not null unique check (room_id = id),
  name text not null check (char_length(name) between 1 and 64),
  os text not null default 'Windows PC',
  created_at timestamptz not null default now()
);

create index if not exists xandesk_devices_owner_id_idx on public.xandesk_devices(owner_id);
alter table public.xandesk_devices enable row level security;

grant select, insert, update, delete on table public.xandesk_devices to authenticated;

drop policy if exists "users manage only their own XanDesk devices" on public.xandesk_devices;
create policy "users manage only their own XanDesk devices"
  on public.xandesk_devices for all to authenticated
  using (owner_id = (select auth.uid()))
  with check (owner_id = (select auth.uid()));
