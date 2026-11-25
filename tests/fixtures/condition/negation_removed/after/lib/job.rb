def go(job)
  return if job.ready?
  job.run
end
