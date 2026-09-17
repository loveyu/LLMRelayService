ALTER TABLE `console_requests` ADD `initial_rate_limit_route_prefix` text;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_rate_limit_target_url` text;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_rate_limit_request_model` text;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_rate_limit_forwarded_payload` text;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_rate_limit_forwarded_headers_json` text;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_rate_limit_response_headers_json` text;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_rate_limit_response_payload` text;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_rate_limit_response_payload_truncated` integer DEFAULT 0 NOT NULL;