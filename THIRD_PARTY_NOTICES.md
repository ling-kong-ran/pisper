# Pisper frontend third-party notices

Pisper (ling-kong-ran/pisper) remains distributed under its MIT license in `LICENSE`.

The greeting layout, outlined Z artwork and dark greeting illustration are adapted from ZCode (zai-org/ZCode), revision `29628c9acdb81b703bbd4080c207a0e7ce5e276e` (3.14.3), under Apache-2.0. The license is reproduced in `LICENSES/Upstream-Greeting-Apache-2.0.txt`. Adapted files: `src/features/chat/WorkbenchGreeting.tsx`, `src/features/chat/workbench-greeting.ts`, `src/assets/pisper-empty-dark.svg`. Changes: Pisper translations, responsive typography, React lifecycle and independent Pisper runtime integration. The sidebar and composer follow its visual layout while retaining Pisper functionality.

Native UI components render their tagged-template markup with htm (developit/htm) 3.1.1, Copyright 2018 Google Inc., under Apache-2.0. It is a pinned dev dependency (`package-lock.json` integrity `sha512-983Vyg8NwUE7JkZ6NmOqpCZ+sh1bKv2iYTlUkzlWmA5JD2acKoxd4KVxbMmxX/85mtfdnDmTFoNKcg5DGAvxNQ==`) bundled unmodified into the lazily loaded component host chunk only (`src/features/custom-ui/native/html.ts`). Its license text, copied from the installed package, is in `LICENSES/htm-Apache-2.0.txt`.

This is an independent Pisper interface, not an official ZCode product. ZCode names and artwork identify the requested visual reference, not ownership or endorsement. All other dependencies retain their respective licenses. No model credentials, user settings, conversations or speech model downloads are bundled by this frontend change.
