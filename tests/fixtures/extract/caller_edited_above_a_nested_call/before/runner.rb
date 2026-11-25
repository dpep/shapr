class Runner
  def run(jobs)
    started = false
    while (batch = jobs.shift)
      if batch
        unless started
          started = true
          puts "start"
        end
        size = batch.size
        puts size
        batch.each { |j| j.call }
        puts "done"
      end
    end
  end
end
