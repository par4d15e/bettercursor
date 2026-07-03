// src/App.tsx — top-level layout: split between SessionTree (left) and SessionDetail (right)

import { SessionTree } from "./components/SessionTree";
import { SessionDetail } from "./components/SessionDetail";

function App() {
  return (
    <div className="flex h-screen w-screen bg-bg-primary text-fg-primary overflow-hidden">
      <div className="w-[40%] min-w-[320px] max-w-[480px]">
        <SessionTree />
      </div>
      <SessionDetail />
    </div>
  );
}

export default App;
