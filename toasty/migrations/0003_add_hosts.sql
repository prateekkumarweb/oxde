CREATE TABLE "hosts" (
    "id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    "name" TEXT NOT NULL,
    "token_id" TEXT NOT NULL,
    "token_hash" TEXT NOT NULL,
    "revoked" BOOLEAN NOT NULL,
    "last_seen_at" BIGINT,
    "created_at" BIGINT NOT NULL,
    "updated_at" BIGINT NOT NULL
);
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX "index_hosts_by_token_id" ON "hosts" ("token_id");
