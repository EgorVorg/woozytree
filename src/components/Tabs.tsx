// Вкладки приложения «Карта | Типы файлов | Поиск» — как в WizTree.
//
// Содержимое вкладок передаётся готовыми узлами: App решает, что показать в
// каждой, а этот компонент отвечает только за переключение (shadcn tabs на
// Base UI). Карта и типы файлов не держатся в DOM, пока открыт поиск, а панель
// поиска живёт постоянно — чтобы фильтры и выдача пережили переход на карту.
import { Tabs as TabsPrimitive } from "@base-ui/react/tabs";
import type { ReactNode } from "react";

/** Вкладка приложения. */
export type AppTab = "map" | "types" | "search";

const TABS: ReadonlyArray<{ value: AppTab; label: string }> = [
  { value: "map", label: "Карта" },
  { value: "types", label: "Типы файлов" },
  { value: "search", label: "Поиск" },
];

function is_app_tab(value: unknown): value is AppTab {
  return value === "map" || value === "types" || value === "search";
}

export interface TabsProps {
  /** Открытая вкладка. */
  value: AppTab;
  /** Переключение вкладки (клик или клавиатура). */
  onChange: (tab: AppTab) => void;
  /** Содержимое вкладки «Карта»: крошки, карта, дерево, таблица файлов. */
  map: ReactNode;
  /** Содержимое вкладки «Типы файлов» (Задача 3). */
  types: ReactNode;
  /** Содержимое вкладки «Поиск». */
  search: ReactNode;
}

/** Переключатель вкладок и их содержимое. */
export default function Tabs({ value, onChange, map, types, search }: TabsProps) {
  return (
    <TabsPrimitive.Root
      value={value}
      onValueChange={(next) => {
        if (is_app_tab(next)) onChange(next);
      }}
      className="flex flex-col gap-4"
    >
      <TabsPrimitive.List
        aria-label="Разделы WoozyTree"
        className="flex flex-wrap items-center gap-1 self-start rounded-lg border border-border bg-card p-1"
      >
        {TABS.map((tab) => (
          <TabsPrimitive.Tab
            key={tab.value}
            value={tab.value}
            className="cursor-pointer rounded-none px-3 py-1.5 text-xs font-medium whitespace-nowrap text-muted-foreground outline-none select-none hover:bg-muted hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring/50 data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-active:bg-primary data-active:text-primary-foreground"
          >
            {tab.label}
          </TabsPrimitive.Tab>
        ))}
      </TabsPrimitive.List>

      <TabsPrimitive.Panel value="map">{map}</TabsPrimitive.Panel>
      <TabsPrimitive.Panel value="types">{types}</TabsPrimitive.Panel>
      {/* Поиск остаётся смонтированным: после перехода к папке на карте фильтры
          и найденные строки не теряются при возврате на вкладку. */}
      <TabsPrimitive.Panel value="search" keepMounted>
        {search}
      </TabsPrimitive.Panel>
    </TabsPrimitive.Root>
  );
}
