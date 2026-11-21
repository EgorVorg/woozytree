# WoozyTree

Клон WizTree для Windows: быстрый сканер дисков, читающий MFT напрямую.

Стек: Tauri 2 (Rust) + React + TypeScript + Vite + Tailwind CSS 4 + shadcn/ui (пресет `b6ZR0yFgO`, тёмная тема).

## Запуск

```powershell
npm install
npm run tauri dev
```

Приложение запускается с правами администратора (манифест `src-tauri/app.manifest`), без этого нельзя читать MFT.
