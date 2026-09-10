ALTER TABLE `console_requests` ADD `initial_response_status` integer DEFAULT 0 NOT NULL;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_response_status_text` text DEFAULT '' NOT NULL;--> statement-breakpoint
ALTER TABLE `console_requests` ADD `initial_completed_at` integer DEFAULT 0 NOT NULL;
--> statement-breakpoint
UPDATE `console_requests`
SET `initial_response_status` = CAST(substr(`failover_reason`, 6, 3) AS integer),
    `initial_response_status_text` = `failover_reason`,
    `original_route_prefix` = CASE
      WHEN instr(`original_route_prefix`, ' (') > 0
        THEN substr(`original_route_prefix`, 1, instr(`original_route_prefix`, ' (') - 1)
      WHEN `original_request_model` IS NOT NULL
        AND substr(`original_route_prefix`, -length(`original_request_model`) - 1) = ':' || `original_request_model`
        THEN substr(`original_route_prefix`, 1, length(`original_route_prefix`) - length(`original_request_model`) - 1)
      ELSE `original_route_prefix`
    END
WHERE `initial_response_status` = 0
  AND `response_status` >= 200
  AND `response_status` < 400
  AND length(`failover_reason`) = 8
  AND `failover_reason` GLOB 'HTTP [0-9][0-9][0-9]';
