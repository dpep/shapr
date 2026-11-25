class Cart
  def total(items)
      items.sum(&:price)
  end
end
