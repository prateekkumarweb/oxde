ALTER TABLE "hosts" ADD COLUMN "last_connected_ip" TEXT;
-- #[toasty::breakpoint]
ALTER TABLE "hosts" ADD COLUMN "ip" TEXT;
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX "index_hosts_by_name" ON "hosts" ("name");
