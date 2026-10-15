import { SettingsMacrosPage } from "@chord/dev.improve.chord.routes.settings.macros.index";
import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/settings/macros/")({ component: SettingsMacrosPage });
