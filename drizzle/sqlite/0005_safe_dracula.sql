CREATE TABLE `concurrency_rules` (
	`id` text PRIMARY KEY NOT NULL,
	`name` text NOT NULL,
	`max_concurrency` integer NOT NULL,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `concurrency_rules_name_unique` ON `concurrency_rules` (`name`);--> statement-breakpoint
ALTER TABLE `console_providers` ADD `concurrency_rule_id` text;