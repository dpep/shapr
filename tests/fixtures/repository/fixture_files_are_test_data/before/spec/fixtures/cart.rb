class Cart
  def total(items)
    sum = 0
    items.each { |item| sum += item.price }
    sum
  end
end
