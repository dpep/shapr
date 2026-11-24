require 'zoo/cart'

RSpec.describe Zoo::Cart do
  it { expect(Zoo::Cart.new).to respond_to(:remove) }
end
