class Batch
  def run(queue)
    drain(queue)
    finish(queue)
  end

  def drain(queue)
    puts "start"
    while (job = queue.pop)
      break if job.stop?
      job.perform
    end
  end
end
