class Batch
  def run(queue)
    puts "start"
    while (job = queue.pop)
      break if job.stop?
      job.perform
    end
    finish(queue)
  end
end
