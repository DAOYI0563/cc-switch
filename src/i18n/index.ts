import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import zh from "./locales/zh.json";

i18n.use(initReactI18next).init({
  resources: { zh: { translation: zh } },
  lng: "zh",
  fallbackLng: "zh",
  supportedLngs: ["zh"],
  interpolation: { escapeValue: false },
  debug: false,
});

export default i18n;
