ALTER TABLE "apps" ADD COLUMN "host_id" BIGINT NOT NULL;
-- #[toasty::breakpoint]
CREATE INDEX "index_apps_by_host_id" ON "apps" ("host_id");
