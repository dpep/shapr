class Cart
  def total(items)
    items.sum(&:price) * 2
  end
end
