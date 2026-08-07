import { useState } from "react"
import { Code2, Cpu, LogOut, Maximize2, Menu, Minimize2, Monitor, Moon, Settings, Sun, SunMoon } from "lucide-react"
import { useTranslation } from "react-i18next"

import { useTheme } from "@/components/theme-provider"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Separator } from "@/components/ui/separator"
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet"
import { useLayoutWidth } from "@/hooks/use-layout-width"
import { cn } from "@/lib/utils"

type Page = "monitor" | "usage" | "providers" | "models" | "routes" | "keys" | "logs" | "settings" | "api" | "system"

const PAGE_SUBTITLE: Record<Page, string> = {
  monitor: "实时流量概览",
  usage: "用量",
  logs: "请求日志",
  providers: "渠道管理",
  models: "模型",
  keys: "密钥管理",
  routes: "路由",
  settings: "配置",
  api: "API 文档",
  system: "Rust 代理",
}

export function NavBar({
  activePage,
  onNavigate,
  logoutPending,
  onLogout,
}: {
  activePage: Page
  onNavigate: (page: Page) => void
  logoutPending: boolean
  onLogout: () => void
}) {
  const { t } = useTranslation()
  const { theme, setTheme } = useTheme()
  const [fullWidth, toggleFullWidth] = useLayoutWidth()
  const [mobileOpen, setMobileOpen] = useState(false)

  // Primary horizontal nav — design order: 监控 用量 日志 渠道 模型 密钥 路由
  const navItems: { page: Page; label: string }[] = [
    { page: "monitor", label: t("nav.monitor") },
    { page: "usage", label: t("nav.usage") },
    { page: "logs", label: t("nav.logs") },
    { page: "providers", label: t("nav.providers") },
    { page: "models", label: t("nav.models") },
    { page: "keys", label: t("nav.keys") },
    { page: "routes", label: t("nav.routes") },
  ]

  const toolItems: { page: Page; label: string; icon: typeof Code2 }[] = [
    { page: "system", label: "代理", icon: Cpu },
    { page: "api", label: "API", icon: Code2 },
    { page: "settings", label: t("nav.settings"), icon: Settings },
  ]

  const iconBtn =
    "flex h-8 w-8 items-center justify-center rounded-[9px] border border-border text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground disabled:opacity-50 disabled:cursor-not-allowed"

  const ThemeIcon = theme === "light" ? Sun : theme === "dark" ? Moon : SunMoon
  const themeTitle =
    theme === "light"
      ? t("nav.themeLight")
      : theme === "dark"
        ? t("nav.themeDark")
        : t("nav.themeSystem")

  const handleNavigate = (page: Page) => {
    setMobileOpen(false)
    onNavigate(page)
  }

  return (
    <header className="flex h-[68px] shrink-0 items-center justify-between gap-3 border-b border-border bg-card px-4 sm:px-5 lg:px-8">
      {/* Brand */}
      <div className="flex items-baseline gap-3">
        <button type="button" onClick={() => handleNavigate("monitor")} className="flex items-center">
          <span className="text-[17px] font-extrabold tracking-[0.04em] text-foreground">LRS</span>
        </button>
        <span className="hidden text-[13px] text-muted-foreground sm:inline">
          {PAGE_SUBTITLE[activePage]}
        </span>
      </div>

      {/* Primary nav — desktop */}
      <nav className="hidden flex-1 items-center justify-center gap-5 text-[13px] md:flex lg:gap-6">
        {navItems.map(({ page, label }) => (
          <button
            key={page}
            type="button"
            onClick={() => onNavigate(page)}
            className={cn(
              "transition-colors",
              activePage === page
                ? "font-semibold text-primary"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {label}
          </button>
        ))}
      </nav>

      {/* Right controls */}
      <div className="flex items-center gap-2.5">
        <div className="hidden items-center gap-1.5 text-[12px] font-semibold text-primary lg:flex">
          <span className="lrs-pulse h-[7px] w-[7px] rounded-full bg-primary" />
          {t("nav.live")}
        </div>

        {/* Mobile nav drawer */}
        <Sheet open={mobileOpen} onOpenChange={setMobileOpen}>
          <SheetTrigger asChild>
            <button
              type="button"
              className={cn(iconBtn, "md:hidden")}
              title={t("nav.menu")}
              aria-label={t("nav.menu")}
            >
              <Menu className="h-[15px] w-[15px]" />
            </button>
          </SheetTrigger>
          <SheetContent side="left" className="w-[280px] p-0">
            <SheetHeader className="px-5 pt-5">
              <SheetTitle className="text-[15px] font-extrabold tracking-[0.04em] text-foreground">
                LRS
              </SheetTitle>
            </SheetHeader>
            <nav className="flex flex-col gap-0.5 px-3 py-2 text-[14px]">
              {navItems.map(({ page, label }) => (
                <button
                  key={page}
                  type="button"
                  onClick={() => handleNavigate(page)}
                  className={cn(
                    "rounded-[9px] px-3 py-2.5 text-left transition-colors",
                    activePage === page
                      ? "bg-accent font-semibold text-accent-foreground"
                      : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                  )}
                >
                  {label}
                </button>
              ))}
            </nav>
            <Separator className="my-1" />
            <div className="flex flex-col gap-0.5 px-3 pb-4 text-[14px]">
              {toolItems.map(({ page, label, icon: Icon }) => (
                <button
                  key={page}
                  type="button"
                  onClick={() => handleNavigate(page)}
                  className={cn(
                    "flex items-center gap-2.5 rounded-[9px] px-3 py-2.5 text-left transition-colors",
                    activePage === page
                      ? "bg-accent font-semibold text-accent-foreground"
                      : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                  )}
                >
                  <Icon className="h-[16px] w-[16px]" />
                  {label}
                </button>
              ))}
            </div>
          </SheetContent>
        </Sheet>

        <button
          type="button"
          onClick={() => onNavigate("system")}
          className={cn(iconBtn, "max-md:hidden", activePage === "system" && "border-primary bg-primary text-primary-foreground")}
          title="Rust 代理"
        >
          <Cpu className="h-[15px] w-[15px]" />
        </button>

        <button
          type="button"
          onClick={() => onNavigate("api")}
          className={cn(iconBtn, "max-md:hidden", activePage === "api" && "border-primary bg-primary text-primary-foreground")}
          title="API"
        >
          <Code2 className="h-[15px] w-[15px]" />
        </button>

        <button
          type="button"
          onClick={() => onNavigate("settings")}
          className={cn(iconBtn, "max-md:hidden", activePage === "settings" && "border-primary bg-primary text-primary-foreground")}
          title={t("nav.settings")}
        >
          <Settings className="h-[15px] w-[15px]" />
        </button>

        {/* Layout width toggle */}
        <button
          type="button"
          onClick={toggleFullWidth}
          className={iconBtn}
          title={fullWidth ? t("nav.layoutConstrained") : t("nav.layoutFullWidth")}
          aria-label={fullWidth ? t("nav.layoutConstrained") : t("nav.layoutFullWidth")}
        >
          {fullWidth ? <Minimize2 className="h-[15px] w-[15px]" /> : <Maximize2 className="h-[15px] w-[15px]" />}
        </button>

        {/* Theme toggle */}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button type="button" className={iconBtn} title={themeTitle} aria-label={t("nav.themeLabel")}>
              <ThemeIcon className="h-[15px] w-[15px]" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-40">
            <DropdownMenuRadioGroup
              value={theme}
              onValueChange={(value) => setTheme(value as "light" | "dark" | "system")}
            >
              <DropdownMenuRadioItem value="light">
                <Sun className="h-4 w-4 text-muted-foreground" />
                {t("nav.themeLight")}
              </DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="dark">
                <Moon className="h-4 w-4 text-muted-foreground" />
                {t("nav.themeDark")}
              </DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="system">
                <Monitor className="h-4 w-4 text-muted-foreground" />
                {t("nav.themeSystem")}
              </DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>

        <button type="button" disabled={logoutPending} onClick={onLogout} className={iconBtn} title={t("nav.logout")}>
          <LogOut className="h-[15px] w-[15px]" />
        </button>
      </div>
    </header>
  )
}
