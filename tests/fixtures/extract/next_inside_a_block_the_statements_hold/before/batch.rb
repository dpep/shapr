class Batch
  def run(items)
    puts "start"
    items.each do |item|
      next if item.nil?
      process(item.to_s)
    end
    finish(items)
  end
end
