class Cart
  def add(item)
    @items << item
  end

  def count
    @items.size
  end

  def empty?
    (@items.empty?)
  end

  private

  def check(item)
    raise ArgumentError unless item
  end
end
