CREATE TABLE "api_tokens" (
    "id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    "user_id" BIGINT NOT NULL,
    "name" TEXT NOT NULL,
    "token_id" TEXT NOT NULL,
    "token_hash" TEXT NOT NULL,
    "expires_at" BIGINT NOT NULL,
    "revoked" BOOLEAN NOT NULL,
    "created_at" BIGINT NOT NULL,
    "updated_at" BIGINT NOT NULL
);
-- #[toasty::breakpoint]
CREATE INDEX "index_api_tokens_by_user_id" ON "api_tokens" ("user_id");
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX "index_api_tokens_by_token_id" ON "api_tokens" ("token_id");
