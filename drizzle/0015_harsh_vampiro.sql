CREATE TABLE "concurrency_rules" (
	"id" text PRIMARY KEY NOT NULL,
	"name" text NOT NULL,
	"max_concurrency" integer NOT NULL,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "concurrency_rules_name_unique" UNIQUE("name")
);
--> statement-breakpoint
ALTER TABLE "console_providers" ADD COLUMN "concurrency_rule_id" text;