import { createWriteStream, mkdirSync, type WriteStream } from 'node:fs';
import { format } from 'node:util';

type LogLevel = 'debug' | 'info' | 'warn' | 'error';

const LEVEL_VALUES: Record<LogLevel, number> = {
  debug: 10,
  info: 20,
  warn: 30,
  error: 40,
};

const originalConsole = {
  log: console.log.bind(console),
  info: console.info.bind(console),
  warn: console.warn.bind(console),
  error: console.error.bind(console),
  debug: console.debug.bind(console),
};

let fileLevel: LogLevel = 'info';
let stdoutLevel: LogLevel = 'warn';
let logDir = '/app/logs';
let currentDate = '';
let stream: WriteStream | null = null;
let initialized = false;
let consoleOverridden = false;
let dateTimeFormatter: Intl.DateTimeFormat;

function parseLevel(value: string | undefined, fallback: LogLevel): LogLevel {
  const normalized = value?.toLowerCase();
  return normalized && normalized in LEVEL_VALUES ? normalized as LogLevel : fallback;
}

function createDateTimeFormatter(timeZone: string): Intl.DateTimeFormat {
  const options: Intl.DateTimeFormatOptions = {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hourCycle: 'h23',
    timeZoneName: 'longOffset',
  };
  try {
    return new Intl.DateTimeFormat('en-US', options);
  } catch {
    originalConsole.warn(`[logger] Invalid TZ '${timeZone}', falling back to UTC`);
    return new Intl.DateTimeFormat('en-US', { ...options, timeZone: 'UTC' });
  }
}

function dateParts(date: Date): Record<string, string> {
  return Object.fromEntries(
    dateTimeFormatter.formatToParts(date).map(({ type, value }) => [type, value]),
  );
}

function dateStamp(date: Date): string {
  const parts = dateParts(date);
  return `${parts.year}-${parts.month}-${parts.day}`;
}

function timestamp(date: Date): string {
  const parts = dateParts(date);
  const offset = parts.timeZoneName === 'GMT' ? 'Z' : parts.timeZoneName?.replace('GMT', '');
  const milliseconds = String(date.getMilliseconds()).padStart(3, '0');
  return `${parts.year}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}:${parts.second}.${milliseconds}${offset}`;
}

function getStream(date: Date): WriteStream {
  const nextDate = dateStamp(date);
  if (stream && currentDate === nextDate) return stream;

  stream?.end();
  currentDate = nextDate;
  stream = createWriteStream(`${logDir}/server.log.${nextDate}`, { flags: 'a' });
  stream.on('error', (error) => {
    originalConsole.error('[logger] Failed to write server log:', error);
  });
  return stream;
}

function log(level: LogLevel, args: unknown[]): void {
  const message = format(...args);
  if (LEVEL_VALUES[level] >= LEVEL_VALUES[fileLevel]) {
    const now = new Date();
    getStream(now).write(`${JSON.stringify({
      timestamp: timestamp(now),
      level,
      message,
    })}\n`);
  }

  if (LEVEL_VALUES[level] >= LEVEL_VALUES[stdoutLevel]) {
    const method = level === 'debug' ? 'debug' : level === 'info' ? 'info' : level;
    originalConsole[method](...args);
  }
}

export function initLoggerFromEnv(): void {
  if (initialized) return;
  logDir = process.env.LOG_DIR || '/app/logs';
  fileLevel = parseLevel(process.env.LOG_LEVEL, 'info');
  stdoutLevel = parseLevel(process.env.LOG_STDOUT_LEVEL, 'warn');
  dateTimeFormatter = createDateTimeFormatter(process.env.TZ || 'UTC');
  mkdirSync(logDir, { recursive: true });
  initialized = true;
}

export function overrideConsole(): void {
  if (consoleOverridden) return;
  if (!initialized) initLoggerFromEnv();

  console.log = (...args: unknown[]) => log('info', args);
  console.info = (...args: unknown[]) => log('info', args);
  console.warn = (...args: unknown[]) => log('warn', args);
  console.error = (...args: unknown[]) => log('error', args);
  console.debug = (...args: unknown[]) => log('debug', args);
  consoleOverridden = true;
}
