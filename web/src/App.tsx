import { Navigate, Route, Routes } from 'react-router-dom'
import { Layout } from './components/Layout'
import { RunsList } from './pages/RunsList'
import { RunNew } from './pages/RunNew'
import { RunDetail } from './pages/RunDetail'
import { Compare } from './pages/Compare'
import { Templates } from './pages/Templates'

export default function App() {
  return (
    <Layout>
      <Routes>
        <Route path="/" element={<Navigate to="/runs" replace />} />
        <Route path="/runs" element={<RunsList />} />
        <Route path="/runs/new" element={<RunNew />} />
        <Route path="/runs/:id" element={<RunDetail />} />
        <Route path="/compare/:idA/:idB" element={<Compare />} />
        <Route path="/templates" element={<Templates />} />
        <Route path="*" element={<Navigate to="/runs" replace />} />
      </Routes>
    </Layout>
  )
}
