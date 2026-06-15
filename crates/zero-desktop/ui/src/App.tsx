import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import ShellLayout from "./shared/ShellLayout";
import EnglishAnnotated from "./modules/english/EnglishAnnotated";
import EnglishAll from "./modules/english/EnglishAll";
import SpeechPage from "./modules/speech/SpeechPage";
import CookiePage from "./modules/cookie/CookiePage";
import NetPolicyPage from "./modules/net-policy/NetPolicyPage";
import SettingsPage from "./modules/settings/SettingsPage";

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<ShellLayout />}>
          <Route index element={<Navigate to="/english/annotated" replace />} />
          <Route path="english/annotated" element={<EnglishAnnotated />} />
          <Route path="english/all" element={<EnglishAll />} />
          <Route path="speech" element={<SpeechPage />} />
          <Route path="cookie" element={<CookiePage />} />
          <Route path="net-policy" element={<NetPolicyPage />} />
          <Route path="settings" element={<SettingsPage />} />
          <Route path="*" element={<Navigate to="/english/annotated" replace />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
