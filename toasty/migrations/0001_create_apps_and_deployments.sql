CREATE TABLE "deployments" (
    "id" BLOB NOT NULL,
    "app_id" BLOB NOT NULL,
    "created_at" BIGINT NOT NULL,
    "original_filename" TEXT,
    "upload_size_bytes" BIGINT NOT NULL,
    "git_info_json" TEXT,
    "build_info_json" TEXT,
    "container_name" TEXT,
    "status" TEXT NOT NULL,
    "failure_error" TEXT,
    PRIMARY KEY ("id")
);
-- #[toasty::breakpoint]
CREATE INDEX "index_deployments_by_app_id" ON "deployments" ("app_id");
-- #[toasty::breakpoint]
CREATE TABLE "app_permissions" (
    "id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    "app_id" BLOB NOT NULL,
    "user_id" BIGINT NOT NULL,
    "level" TEXT NOT NULL,
    "created_at" BIGINT NOT NULL,
    "updated_at" BIGINT NOT NULL
);
-- #[toasty::breakpoint]
CREATE INDEX "index_app_permissions_by_app_id" ON "app_permissions" ("app_id");
-- #[toasty::breakpoint]
CREATE INDEX "index_app_permissions_by_user_id" ON "app_permissions" ("user_id");
-- #[toasty::breakpoint]
CREATE TABLE "apps" (
    "id" BLOB NOT NULL,
    "name" TEXT NOT NULL,
    "source_json" TEXT NOT NULL,
    "env_vars_json" TEXT NOT NULL,
    "active_deployment_id" BLOB,
    "created_at" BIGINT NOT NULL,
    "updated_at" BIGINT NOT NULL,
    PRIMARY KEY ("id")
);
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX "index_apps_by_name" ON "apps" ("name");
