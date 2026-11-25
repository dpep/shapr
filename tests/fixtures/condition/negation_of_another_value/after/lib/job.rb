def go(job)
  return if !job.done?
  job.run
end
