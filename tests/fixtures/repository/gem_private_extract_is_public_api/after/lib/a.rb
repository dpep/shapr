class Printer
  def run(items)
    puts "start"
    show(items)
    puts "done"
  end

  private

  def show(items)
    items.each { |i| puts i * 2 }
  end
end
